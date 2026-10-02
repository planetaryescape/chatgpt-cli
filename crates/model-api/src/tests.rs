#![allow(clippy::unwrap_used)]

use serde_json::json;
use wiremock::matchers::{body_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const SENTINEL: &str = "SENTINEL private transcript";

fn key(value: &str) -> ApiKey {
    ApiKey::new(value, "OpenAI").unwrap().unwrap()
}

#[tokio::test]
async fn openai_gets_the_ts_clis_request_and_its_text_is_read() {
    let server = MockServer::start().await;
    let schema = json!({ "type": "object", "required": ["a"] });
    let expected = r#"{"model":"gpt-6-luna","reasoning":{"effort":"medium"},"store":false,"instructions":"Do it.","input":"{\"x\":1}","text":{"format":{"type":"json_schema","name":"chatgpt_cli_result","strict":true,"schema":{"type":"object","required":["a"]}}}}"#;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer sk-test"))
        .and(header("content-type", "application/json"))
        .and(body_string(expected))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed",
            "output": [
                { "type": "reasoning", "content": [{ "type": "output_text", "text": "no" }] },
                { "type": "message", "content": [
                    { "type": "output_text", "text": "{\"a\":" },
                    { "type": "refusal", "text": "x" },
                    { "type": "output_text", "text": "1}" },
                ] },
            ],
            "usage": { "input_tokens": 1000, "output_tokens": 50, "input_tokens_details": { "cached_tokens": 400 } },
        })))
        .expect(1)
        .mount(&server)
        .await;
    let openai = OpenAi::new(&server.uri(), key(" sk-test ")).unwrap();
    let reply = openai
        .text("Do it.", r#"{"x":1}"#, Some(&schema))
        .await
        .unwrap();
    assert_eq!(
        reply,
        Reply {
            text: "{\"a\":1}".into(),
            input_tokens: 1000,
            cached_input_tokens: 400,
            output_tokens: 50
        }
    );
    assert!(!format!("{openai:?}").contains("sk-test"));
}

#[tokio::test]
async fn errors_never_quote_the_answer() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_string(SENTINEL))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "failed", "error": { "message": SENTINEL },
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "status": SENTINEL })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!("{{{SENTINEL}")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "completed", "output": [],
        })))
        .mount(&server)
        .await;
    let openai = OpenAi::new(&server.uri(), key("k")).unwrap();
    let mut messages = Vec::new();
    for _ in 0..5 {
        messages.push(openai.text("i", "x", None).await.unwrap_err().to_string());
    }
    for message in &messages {
        assert!(!message.contains("SENTINEL"), "{message}");
    }
    assert_eq!(
        messages,
        [
            "127.0.0.1 returned 400",
            "OpenAI response failed: no completed output",
            "OpenAI response unexpected: no completed output",
            "127.0.0.1 answered with unreadable JSON (Syntax error)",
            "OpenAI returned no text.",
        ]
    );
}

#[tokio::test]
async fn anthropic_gets_the_ts_clis_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "ant-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_string(
            r#"{"model":"claude-haiku-4-5","max_tokens":1800,"system":"Sum.","messages":[{"role":"user","content":"Title: t\n\nhi"}]}"#,
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{ "type": "text", "text": "A summary." }, { "type": "tool_use" }],
            "usage": { "input_tokens": 2000, "output_tokens": 100 },
            "stop_reason": "end_turn",
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{ "type": "text", "text": SENTINEL }], "stop_reason": "max_tokens",
        })))
        .mount(&server)
        .await;
    let anthropic = Anthropic::new(
        &server.uri(),
        ApiKey::new("ant-test", "Anthropic").unwrap().unwrap(),
    )
    .unwrap();
    let reply = anthropic.text("Sum.", "Title: t\n\nhi").await.unwrap();
    assert_eq!(reply.text, "A summary.");
    assert_eq!((reply.input_tokens, reply.output_tokens), (2000, 100));
    assert_eq!(
        anthropic.text("Sum.", "x").await.unwrap_err().to_string(),
        "Anthropic summary was truncated at max_tokens."
    );
}

#[test]
fn a_key_that_cant_be_a_header_is_refused_unseen() {
    let error = ApiKey::new("SENTINEL\nkey", "OpenAI").unwrap_err();
    assert!(!error.to_string().contains("SENTINEL"));
    assert!(ApiKey::new("   ", "OpenAI").unwrap().is_none());
}
