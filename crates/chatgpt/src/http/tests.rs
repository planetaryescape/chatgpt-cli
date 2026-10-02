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
async fn honours_a_retry_after_within_the_limit() {
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
fn parses_retry_after_seconds_and_ignores_unusable_values() {
    assert_eq!(parse_retry_after("7"), Some(Duration::from_secs(7)));
    assert_eq!(parse_retry_after(" 0.5 "), Some(ms(500)));
    // Past u64 seconds: saturates instead of panicking.
    assert_eq!(
        parse_retry_after("18446744073709551616"),
        Some(Duration::MAX)
    );
    assert_eq!(parse_retry_after("1e300"), Some(Duration::MAX));
    for unusable in ["Wed, 21 Oct 2026 07:28:00 GMT", "0", "-3", "NaN", "inf", ""] {
        assert_eq!(parse_retry_after(unusable), None, "{unusable}");
    }
    let policy = RetryPolicy::default();
    assert_eq!(
        policy.backoff(RetryReason::RateLimited, 1),
        Duration::from_secs(10)
    );
    let gateway = RetryReason::Gateway(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(policy.backoff(gateway, 3), Duration::from_secs(8));
}

#[tokio::test]
async fn stops_instead_of_retrying_early_when_429_asks_for_too_long() {
    for retry_after in ["3600", "18446744073709551616"] {
        let server = MockServer::start().await;
        let limited = ResponseTemplate::new(429).insert_header("retry-after", retry_after);
        mount_then_ok(&server, limited, 1).await;
        let (http, events) = client(&server, fast_policy());
        let error = http.get("/x", &[]).await.unwrap_err();
        assert!(matches!(error, HttpError::RateLimited { .. }), "{error:?}");
        assert!(
            error.to_string().starts_with("rate limited by ChatGPT"),
            "{error}"
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn stops_on_a_gateway_error_that_asks_for_too_long() {
    let server = MockServer::start().await;
    let down = ResponseTemplate::new(503).insert_header("retry-after", "3600");
    mount_then_ok(&server, down, 1).await;
    let (http, _) = client(&server, fast_policy());
    let error = http.get("/x", &[]).await.unwrap_err();
    assert!(
        matches!(
            error,
            HttpError::Status {
                status: StatusCode::SERVICE_UNAVAILABLE,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

const SENTINEL: &str = "SENTINEL-access-token-value";

fn assert_hidden(error: &(impl std::fmt::Debug + std::fmt::Display)) {
    let printed = format!("{error} {error:?}");
    assert!(!printed.contains(SENTINEL), "{printed}");
}

#[tokio::test]
async fn never_echoes_a_malformed_session_body() {
    // Valid JSON of the wrong shape: serde's message would quote the string.
    for body in [
        format!(r#""{SENTINEL}""#),
        format!(r#"{{"accessToken":["{SENTINEL}"]}}"#),
        format!("{SENTINEL} not json"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(SESSION_PATH))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let (http, _) = client(&server, fast_policy());
        let error = exchange_session(&http, "dia").await.unwrap_err();
        assert!(matches!(error, SessionError::Malformed { .. }), "{error:?}");
        assert_hidden(&error);
    }
}

#[tokio::test]
async fn never_echoes_an_auth_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SESSION_PATH))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(format!(r#"{{"accessToken":"{SENTINEL}"}}"#)),
        )
        .mount(&server)
        .await;
    let (http, _) = client(&server, fast_policy());
    let error = exchange_session(&http, "dia").await.unwrap_err();
    assert!(
        matches!(
            error,
            SessionError::Http(HttpError::Status { snippet: None, .. })
        ),
        "{error:?}"
    );
    assert_hidden(&error);
}

#[tokio::test]
async fn the_token_names_the_account_it_belongs_to() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SESSION_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"accessToken":"t","user":{"id":"user-abc","email":"x"}}"#),
        )
        .mount(&server)
        .await;
    let (http, _) = client(&server, fast_policy());
    let token = exchange_session(&http, "dia").await.unwrap();
    assert_eq!(token.account(), Some("user-abc"));
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
    assert_eq!(token.account(), None, "`user` without an id");
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

/// Answers `first` `times` times to `method /w`, then 200 "done".
async fn mount_write_then_ok(server: &MockServer, verb: &str, first: ResponseTemplate, times: u64) {
    Mock::given(method(verb))
        .and(path("/w"))
        .respond_with(first)
        .up_to_n_times(times)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method(verb))
        .and(path("/w"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .with_priority(2)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_write_that_must_not_repeat_is_sent_once_past_a_gateway_error() {
    for (verb, method_) in [("DELETE", HttpMethod::Delete), ("POST", HttpMethod::Post)] {
        let server = MockServer::start().await;
        mount_write_then_ok(&server, verb, ResponseTemplate::new(502), 1).await;
        let (http, events) = client(&server, fast_policy());
        let error = http
            .send_with(method_, "/w", &[], Some("{}"), Resend::OnlyIfRefused)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, HttpError::Status { status, .. } if status.as_u16() == 502),
            "{error:?}"
        );
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn a_write_that_must_not_repeat_still_retries_a_refusal() {
    for refusal in [challenge(), ResponseTemplate::new(429)] {
        let server = MockServer::start().await;
        mount_write_then_ok(&server, "PATCH", refusal, 1).await;
        let (http, events) = client(&server, fast_policy());
        let body = http
            .send_with(
                HttpMethod::Patch,
                "/w",
                &[],
                Some("{}"),
                Resend::OnlyIfRefused,
            )
            .await
            .unwrap();
        assert_eq!(body, "done");
        assert_eq!(events.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn an_idempotent_write_retries_gateway_errors() {
    let server = MockServer::start().await;
    mount_write_then_ok(&server, "PATCH", ResponseTemplate::new(503), 1).await;
    let (http, _) = client(&server, fast_policy());
    let body = http
        .send(HttpMethod::Patch, "/w", &[], Some("{}"))
        .await
        .unwrap();
    assert_eq!(body, "done");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
