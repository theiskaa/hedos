//! The Anthropic messages handler end to end against a mock port: the SSE event
//! grammar, tool-use blocks, the unary shape, and error bodies.

mod common;

use common::MockPort;
use gateway::error::GatewayErrorKind;
use gateway::handlers::GatewayHandling;
use gateway::handlers::messages::AnthropicMessagesHandler;
use gateway::identity::GatewayIdentity;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::scopes::GatewayScopes;
use kernel::capabilities::{CapabilityChunk, GenerationStats, ToolCall};
use kernel::records::JsonValue;

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(body: &str) -> GatewayRequest {
    GatewayRequest::new("POST", "/v1/messages", Vec::new(), body.as_bytes().to_vec())
}

fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> (Option<u16>, String) {
    let mut status = None;
    let mut body = Vec::new();
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status: code, .. } => status = Some(code),
            ResponsePart::Chunk(bytes) => body.extend(bytes),
        }
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn hello_port() -> MockPort {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.chunks = vec![
        CapabilityChunk::Text("Hello".to_owned()),
        CapabilityChunk::Text(" world".to_owned()),
        CapabilityChunk::Done(Some(GenerationStats {
            prompt_tokens: Some(2),
            completion_tokens: Some(2),
            ..Default::default()
        })),
    ];
    port
}

/// The order of `event:` lines in a stream, which is the part clients parse.
fn events(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_streamed_message_follows_the_anthropic_event_grammar() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":true}"#;
    AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();

    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    assert_eq!(
        events(&out),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert!(out.contains("\"type\":\"text_delta\""));
    assert!(out.contains("Hello"));
    assert!(out.contains("world"));
    assert!(out.contains("\"stop_reason\":\"end_turn\""));
    // Anthropic has no [DONE] sentinel; message_stop terminates the stream.
    assert!(!out.contains("[DONE]"));
}

#[tokio::test]
async fn a_tool_call_closes_the_text_block_and_reports_tool_use() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.shelf[0]
        .capabilities
        .push(kernel::records::Capability::tools());
    port.chunks = vec![
        CapabilityChunk::Text("checking".to_owned()),
        CapabilityChunk::ToolCall(ToolCall {
            id: "toolu_1".to_owned(),
            name: "read".to_owned(),
            arguments: JsonValue::Object(Default::default()),
        }),
        CapabilityChunk::Done(None),
    ];
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],"stream":true,
        "tools":[{"name":"read","description":"read a file","input_schema":{"type":"object"}}]}"#;
    AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();

    let (_, out) = collect(rx);
    // The text block must be closed before the tool block opens, and the two
    // blocks must carry distinct indices.
    assert_eq!(
        events(&out),
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert!(out.contains("\"type\":\"tool_use\""));
    assert!(out.contains("\"id\":\"toolu_1\""));
    assert!(out.contains("\"type\":\"input_json_delta\""));
    assert!(out.contains("\"index\":1"));
    // Claude Code drives its agent loop off tool_use, not end_turn.
    assert!(out.contains("\"stop_reason\":\"tool_use\""));
}

#[tokio::test]
async fn a_unary_message_returns_one_json_body() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}]}"#;
    AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();

    let (status, out) = collect(rx);
    assert_eq!(status, Some(200));
    let value: serde_json::Value = serde_json::from_str(&out).expect("valid json");
    assert_eq!(value["type"], "message");
    assert_eq!(value["role"], "assistant");
    assert_eq!(value["content"][0]["text"], "Hello world");
    assert_eq!(value["stop_reason"], "end_turn");
    assert_eq!(value["usage"]["input_tokens"], 2);
    assert_eq!(value["usage"]["output_tokens"], 2);
    assert!(
        value["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("msg_"))
    );
}

#[tokio::test]
async fn the_fields_claude_code_sends_uninvited_do_not_fail_the_request() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    // Claude Code sends all of these to any model name it does not recognize,
    // which is every alias hedos serves.
    let body = r#"{"model":"llama3","messages":[{"role":"user","content":"hi"}],
        "thinking":{"type":"adaptive"},
        "context_management":{"edits":[]},
        "output_config":{"effort":"high"},
        "metadata":{"user_id":"x"}}"#;
    AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();

    let (status, _) = collect(rx);
    assert_eq!(status, Some(200));
}

#[tokio::test]
async fn a_system_prompt_and_a_tool_result_reach_the_model() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let body = r#"{"model":"llama3",
        "system":[{"type":"text","text":"be terse"}],
        "messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"read","input":{}}]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_1","content":"file body"}]}
        ]}"#;
    AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .unwrap();

    let (status, _) = collect(rx);
    assert_eq!(status, Some(200));
}

#[tokio::test]
async fn an_unknown_model_reports_an_anthropic_shaped_error() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let (responder, _rx) = GatewayResponder::new();
    let body = r#"{"model":"nope","messages":[{"role":"user","content":"hi"}]}"#;
    let error = AnthropicMessagesHandler::default()
        .handle(&request(body), &identity(), &port, &responder)
        .await
        .expect_err("an unknown model is an error");
    assert_eq!(error.kind, GatewayErrorKind::NotFound);

    // Claude Code string-matches on the upstream's wording to decide whether to
    // retry, so the body must be Anthropic's shape, not a wrapper.
    let value = error.body(gateway::surface::GatewaySurface::Anthropic);
    assert_eq!(value["type"], "error");
    assert_eq!(value["error"]["type"], "not_found_error");
    assert!(value["error"]["message"].as_str().is_some());
}

#[tokio::test]
async fn a_malformed_request_is_rejected() {
    let port = hello_port();
    let (responder, rx) = GatewayResponder::new();
    let error = AnthropicMessagesHandler::default()
        .handle(
            &request(r#"{"messages":[]}"#),
            &identity(),
            &port,
            &responder,
        )
        .await
        .expect_err("a missing model is an error");
    assert_eq!(error.kind, GatewayErrorKind::BadRequest);
    let _ = collect(rx);
}
