#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::session::{SESSION_PATH, SessionError, exchange_session};

const COOKIE: &str = "__Secure-next-auth.session-token=secret-cookie";

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// The production policy shape, scaled from seconds to milliseconds.
fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        max_retries: 4,
        base_backoff: ms(1),
        rate_limit_backoff: ms(5),
        max_retry_after: ms(60),
    }
}

fn client(server: &MockServer, policy: RetryPolicy) -> (HttpClient, Arc<Mutex<Vec<RetryEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&events);
    let http = HttpClient::new(server.uri(), Secret::new(COOKIE.to_owned()), policy)
        .unwrap()
        .on_retry(move |event| seen.lock().unwrap().push(*event));
    (http, events)
}

fn challenge() -> ResponseTemplate {
    ResponseTemplate::new(403).insert_header("cf-mitigated", "challenge")
}

/// Answers `first` `times` times, then 200 "ok".
async fn mount_then_ok(server: &MockServer, first: ResponseTemplate, times: u64) {
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(first)
        .up_to_n_times(times)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .with_priority(2)
        .mount(server)
        .await;
}

fn reasons_and_waits(events: &Mutex<Vec<RetryEvent>>) -> Vec<(RetryReason, Duration)> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|event| (event.reason, event.wait))
        .collect()
}

#[tokio::test]
async fn retries_a_challenge_on_a_fresh_client() {
    let server = MockServer::start().await;
    mount_then_ok(&server, challenge(), 2).await;
    let (http, events) = client(&server, fast_policy());
    assert_eq!(http.get("/x", &[]).await.unwrap(), "ok");
    assert_eq!(
        reasons_and_waits(&events),
        [
            (RetryReason::Challenge, ms(1)),
            (RetryReason::Challenge, ms(2))
        ]
    );
}

#[tokio::test]
async fn backs_off_longer_on_429_than_on_gateway_errors() {
    let server = MockServer::start().await;
    mount_then_ok(&server, ResponseTemplate::new(429), 3).await;
    let (http, events) = client(&server, fast_policy());
    http.get("/x", &[]).await.unwrap();
    let limited = RetryReason::RateLimited;
    assert_eq!(
        reasons_and_waits(&events),
        [(limited, ms(5)), (limited, ms(10)), (limited, ms(20))]
    );
}

#[tokio::test]
async fn retries_each_gateway_error() {
    for status in [502, 503, 504] {
        let server = MockServer::start().await;
        mount_then_ok(&server, ResponseTemplate::new(status), 1).await;
        let (http, events) = client(&server, fast_policy());
        assert_eq!(http.get("/x", &[]).await.unwrap(), "ok");
        let code = StatusCode::from_u16(status).unwrap();
        assert_eq!(
            reasons_and_waits(&events),
            [(RetryReason::Gateway(code), ms(1))]
        );
    }
}

#[tokio::test]
async fn never_retries_a_500_or_a_plain_403() {
    for status in [500, 403] {
        let server = MockServer::start().await;
        mount_then_ok(
            &server,
            ResponseTemplate::new(status).set_body_string("nope"),
            1,
        )
        .await;
        let (http, events) = client(&server, fast_policy());
        let error = http.get("/x", &[]).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "{status} {} from /x: nope",
                StatusCode::from_u16(status)
                    .unwrap()
                    .canonical_reason()
                    .unwrap()
            )
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn gives_up_after_four_retries() {
    let server = MockServer::start().await;
    mount_then_ok(&server, challenge(), 100).await;
    let (http, events) = client(&server, fast_policy());
    let error = http.get("/x", &[]).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Cloudflare kept challenging requests to /x. Wait a minute and retry."
    );
    assert_eq!(events.lock().unwrap().len(), 4);
    assert_eq!(server.received_requests().await.unwrap().len(), 5);
}

#[tokio::test]
async fn honours_retry_after() {
    let server = MockServer::start().await;
    mount_then_ok(
        &server,
        ResponseTemplate::new(503).insert_header("retry-after", "0.03"),
        1,
    )
    .await;
    let (http, events) = client(&server, fast_policy());
    http.get("/x", &[]).await.unwrap();
    assert_eq!(reasons_and_waits(&events)[0].1, ms(30));
}

#[test]
fn clamps_retry_after_and_ignores_unusable_values() {
    let policy = RetryPolicy::default();
    let wait = |status: u16, attempt, retry_after| {
        policy.wait(StatusCode::from_u16(status).unwrap(), attempt, retry_after)
    };
    assert_eq!(wait(429, 0, Some("7")), Duration::from_secs(7));
    assert_eq!(wait(429, 0, Some("3600")), Duration::from_secs(60));
    // An HTTP date, zero or garbage falls back to the backoff.
    assert_eq!(
        wait(429, 1, Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        Duration::from_secs(10)
    );
    assert_eq!(wait(503, 2, Some("0")), Duration::from_secs(4));
    assert_eq!(wait(503, 3, None), Duration::from_secs(8));
    assert_eq!(wait(429, 4, Some("NaN")), Duration::from_secs(80));
}

#[tokio::test]
async fn sends_the_browser_cookies_and_exchanges_them_for_a_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SESSION_PATH))
        .and(header("cookie", COOKIE))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"accessToken":"secret-token","user":{}}"#),
        )
        .mount(&server)
        .await;
    let (http, _) = client(&server, fast_policy());
    let token = exchange_session(&http, "dia profile \"Default\"")
        .await
        .unwrap();
    assert_eq!(token.expose(), "secret-token");
    let printed = format!("{token:?} {http:?}");
    assert!(
        !printed.contains("secret-token") && !printed.contains("secret-cookie"),
        "{printed}"
    );
}

#[tokio::test]
async fn reports_an_expired_browser_session() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SESSION_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let (http, _) = client(&server, fast_policy());
    let error = exchange_session(&http, "dia profile \"Default\"")
        .await
        .unwrap_err();
    assert!(matches!(error, SessionError::Expired { .. }));
    assert_eq!(
        error.to_string(),
        "ChatGPT session in dia profile \"Default\" has expired. Open chatgpt.com there to refresh it, then retry."
    );
}

#[test]
fn rejects_a_cookie_header_without_echoing_it() {
    let error = HttpClient::new(
        "http://localhost",
        Secret::new("a=secret\nvalue".to_owned()),
        RetryPolicy::default(),
    )
    .unwrap_err();
    assert!(!error.to_string().contains("secret"), "{error}");
}

#[test]
fn the_client_can_be_shared_across_tasks() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HttpClient>();
}
