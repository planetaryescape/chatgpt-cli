#![allow(clippy::unwrap_used)]

use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn fast() -> RetryPolicy {
    RetryPolicy {
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(5),
        timeout: Duration::from_millis(500),
        ..RetryPolicy::default()
    }
}

fn client(server: &MockServer) -> Client {
    Client::new(
        &server.uri(),
        ApiKey::new(" key-123 ").unwrap().unwrap(),
        fast(),
    )
    .unwrap()
}

fn answer() -> ResponseTemplate {
    // Key order and an exact float, as Jev sends them.
    ResponseTemplate::new(200).set_body_raw(
        r#"{"answers":{"b":{"type":"noul","noul":0.9400000000000001},"a":{"type":"choice","choice":"x","confidence":1e-7}},"usage":{"input_tokens":1234},"model":"jev-1"}"#,
        "application/json",
    )
}

#[tokio::test]
async fn posts_the_sdk_body_and_keeps_the_answers_as_sent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer key-123"))
        .and(header("content-type", "application/json"))
        .respond_with(answer())
        .expect(1)
        .mount(&server)
        .await;
    let state = json!({ "conversation": { "title": "T" }, "content": "c" });
    let questions = json!({ "q": { "type": "noul", "instructions": "?" } });
    let result = client(&server)
        .system_one(&state, &questions)
        .await
        .unwrap();
    assert_eq!(result.input_tokens, 1234);
    assert_eq!(
        serde_json::to_string(&result.answers).unwrap(),
        r#"{"b":{"type":"noul","noul":0.9400000000000001},"a":{"type":"choice","choice":"x","confidence":1e-7}}"#
    );
    let sent = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        String::from_utf8(sent.body.clone()).unwrap(),
        r#"{"state":{"conversation":{"title":"T"},"content":"c"},"questions":{"q":{"type":"noul","instructions":"?"}},"model":"jev-latest"}"#
    );
}

#[tokio::test]
async fn retries_429_and_5xx_twice_then_reports_the_status_without_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("secret echo of the transcript"))
        .mount(&server)
        .await;
    let error = client(&server)
        .system_one(&json!({}), &json!({}))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "TypeSafe answered 503");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    let retried = &server.received_requests().await.unwrap()[2];
    assert_eq!(retried.headers.get("x-typesafe-retry-count").unwrap(), "2");
}

#[tokio::test]
async fn a_rejected_key_is_not_retried_and_says_where_to_fix_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("{\"detail\":\"bad key\"}"))
        .mount(&server)
        .await;
    let error = client(&server)
        .system_one(&json!({}), &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Status { status: 401 }));
    assert!(!error.to_string().contains("bad key"), "{error}");
    assert!(error.to_string().contains("chatgpt configure jev"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn honours_a_short_retry_after_and_recovers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after-ms", "20"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(answer())
        .with_priority(2)
        .mount(&server)
        .await;
    let started = std::time::Instant::now();
    client(&server)
        .system_one(&json!({}), &json!({}))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(20));
}

#[tokio::test]
async fn a_slow_answer_times_out_after_the_retries() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer().set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let error = client(&server)
        .system_one(&json!({}), &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)), "{error:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn an_answer_of_another_shape_is_a_decode_error_without_its_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"secret\": true}"))
        .mount(&server)
        .await;
    let error = client(&server)
        .system_one(&json!({}), &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Decode(_)));
    assert!(!error.to_string().contains("secret"));
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let key = ApiKey::new("ts-live-abcdef").unwrap().unwrap();
    assert_eq!(format!("{key:?}"), "ApiKey(<redacted>)");
    let client = Client::new("http://127.0.0.1:9/", key, RetryPolicy::default()).unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("abcdef"), "{debug}");
    assert!(ApiKey::new("  ").unwrap().is_none());
}

#[test]
fn a_key_that_cant_be_a_header_is_refused_without_being_echoed() {
    for sentinel in ["SENTINEL-ab\ncd-KEY", "SENTINEL-ab\rcd", "SENTINEL\u{7f}x"] {
        let error = ApiKey::new(sentinel).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("SENTINEL"), "{shown}");
    }
    let error = transport_error(
        &impit::errors::ImpitError::InvalidHeaderValue("Bearer SENTINEL".into()),
        Duration::from_secs(1),
    );
    assert!(!error.to_string().contains("SENTINEL"), "{error}");
}
