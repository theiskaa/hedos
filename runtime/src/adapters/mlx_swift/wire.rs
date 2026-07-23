//! The wire forms of the MLX-Swift shim ABI: the request JSON `hedos_mlx_stream`
//! takes and the event payloads it emits. Pure functions, so the FFI protocol
//! stays testable on every platform even though the FFI itself is compiled only
//! on macOS.

use kernel::capabilities::{ChatMessage, ToolCall, ToolSpec};
use kernel::records::JsonValue;
use serde_json::{Map, Value, json};

use super::backend::{MlxSwiftDone, MlxSwiftEvent, MlxSwiftOptions};

/// The request `hedos_mlx_stream` takes: the model directory to load, the
/// messages, the tools on offer, and only the sampling options actually set.
/// Messages, tool specs, and calls go through their canonical `payload_value`
/// forms so the wire shape is pinned to those, not to the structs' derived
/// serialization.
pub(crate) fn request_json(
    model_dir: &str,
    messages: &[ChatMessage],
    tools: &[ToolSpec],
    options: &MlxSwiftOptions,
) -> String {
    let wire_messages: Vec<Value> = messages
        .iter()
        .map(|message| serde_value(&message.payload_value()))
        .collect();
    let mut object = Map::new();
    object.insert("model".to_owned(), Value::String(model_dir.to_owned()));
    object.insert("messages".to_owned(), Value::Array(wire_messages));
    if !tools.is_empty() {
        let wire_tools = tools
            .iter()
            .map(|tool| serde_value(&tool.payload_value()))
            .collect();
        object.insert("tools".to_owned(), Value::Array(wire_tools));
    }
    if let Some(temperature) = options.temperature {
        object.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(top_p) = options.top_p {
        object.insert("top_p".to_owned(), json!(top_p));
    }
    if let Some(repeat_penalty) = options.repeat_penalty {
        object.insert("repeat_penalty".to_owned(), json!(repeat_penalty));
    }
    if let Some(max_tokens) = options.max_tokens {
        object.insert("max_tokens".to_owned(), json!(max_tokens));
    }
    if !options.stop.is_empty() {
        object.insert(
            "stop".to_owned(),
            Value::Array(options.stop.iter().map(|s| json!(s)).collect()),
        );
    }
    Value::Object(object).to_string()
}

/// A kernel [`JsonValue`] as a `serde_json` value. The fallback is unreachable —
/// every `JsonValue` key is a string.
fn serde_value(value: &JsonValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Parse a tool-call-event payload (`{"id"?,"name","arguments"?:{…}}`) into the
/// event, or `None` for a payload without a usable call — a nameless or
/// malformed one is dropped, like any other unparseable frame on this wire. An
/// absent `arguments` defaults to `{}`, but a present non-object (null included)
/// drops the call — the shim must send an object or omit the key, never null.
pub(crate) fn tool_call_event(payload: &str) -> Option<MlxSwiftEvent> {
    let value: JsonValue = serde_json::from_str(payload).ok()?;
    ToolCall::from_payload(&value)
        .filter(|call| !call.name.is_empty())
        .map(MlxSwiftEvent::ToolCall)
}

/// Parse a done-event payload into the terminal event; malformed payloads
/// become a done with no counts rather than an error.
pub(crate) fn done_event(payload: &str) -> MlxSwiftEvent {
    let value: Option<Value> = serde_json::from_str(payload).ok();
    let field = |key: &str| value.as_ref().and_then(|v| v.get(key));
    let count = |key: &str| field(key).and_then(Value::as_i64);
    MlxSwiftEvent::Done(MlxSwiftDone {
        prompt_tokens: count("prompt_tokens"),
        completion_tokens: count("completion_tokens"),
        load_ms: count("load_ms"),
        finish_reason: field("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
        token_counts_estimated: field("token_counts_estimated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::capabilities::ChatRole;

    #[test]
    fn the_request_carries_the_model_messages_and_only_the_set_options() {
        let messages = vec![
            ChatMessage::new(ChatRole::System, "be brief"),
            ChatMessage::new(ChatRole::User, "hi"),
        ];
        let options = MlxSwiftOptions {
            temperature: Some(0.5),
            repeat_penalty: Some(1.1),
            stop: vec!["<end>".to_owned()],
            ..MlxSwiftOptions::default()
        };
        let parsed: Value =
            serde_json::from_str(&request_json("/models/qwen", &messages, &[], &options)).unwrap();
        assert_eq!(parsed["model"], "/models/qwen");
        assert_eq!(parsed["messages"][0]["role"], "system");
        assert_eq!(parsed["messages"][1]["content"], "hi");
        assert_eq!(parsed["temperature"], 0.5);
        assert_eq!(parsed["repeat_penalty"], 1.1);
        assert_eq!(parsed["stop"][0], "<end>");
        assert!(parsed.get("tools").is_none());
        assert!(parsed.get("top_p").is_none());
        assert!(parsed.get("max_tokens").is_none());
    }

    #[test]
    fn the_request_carries_tools_and_tool_routed_messages() {
        let mut assistant = ChatMessage::new(ChatRole::Assistant, "");
        assistant.tool_calls = vec![ToolCall::with_id(
            "call_1",
            "read",
            JsonValue::Object(
                [("path".to_owned(), JsonValue::String("a.txt".to_owned()))]
                    .into_iter()
                    .collect(),
            ),
        )];
        let mut result = ChatMessage::new(ChatRole::Tool, "contents");
        result.tool_call_id = Some("call_1".to_owned());
        result.tool_name = Some("read".to_owned());
        let messages = vec![
            assistant,
            result,
            ChatMessage::new(ChatRole::User, "and now?"),
        ];
        let tools = vec![ToolSpec::new(
            "read",
            "read a file",
            JsonValue::Object(
                [("type".to_owned(), JsonValue::String("object".to_owned()))]
                    .into_iter()
                    .collect(),
            ),
        )];
        let parsed: Value = serde_json::from_str(&request_json(
            "/m",
            &messages,
            &tools,
            &MlxSwiftOptions::default(),
        ))
        .unwrap();
        assert_eq!(parsed["tools"][0]["name"], "read");
        assert_eq!(parsed["tools"][0]["parameters"]["type"], "object");
        assert_eq!(parsed["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            parsed["messages"][0]["tool_calls"][0]["arguments"]["path"],
            "a.txt"
        );
        assert_eq!(parsed["messages"][1]["role"], "tool");
        assert_eq!(parsed["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(parsed["messages"][1]["tool_name"], "read");
    }

    #[test]
    fn tool_call_payloads_parse_mint_ids_and_drop_the_malformed() {
        let Some(MlxSwiftEvent::ToolCall(call)) =
            tool_call_event(r#"{"id":"call_9","name":"read","arguments":{"path":"a"}}"#)
        else {
            panic!("expected a tool call");
        };
        assert_eq!(call.id, "call_9");
        assert_eq!(call.name, "read");
        let Some(MlxSwiftEvent::ToolCall(minted)) = tool_call_event(r#"{"name":"read"}"#) else {
            panic!("expected a tool call with a minted id");
        };
        assert!(!minted.id.is_empty());
        assert_eq!(tool_call_event(r#"{"arguments":{}}"#), None);
        assert_eq!(tool_call_event(r#"{"name":""}"#), None);
        assert_eq!(tool_call_event(r#"{"name":"read","arguments":3}"#), None);
        assert_eq!(tool_call_event(r#"{"name":"read","arguments":null}"#), None);
        assert_eq!(tool_call_event("not json"), None);
    }

    #[test]
    fn done_payloads_parse_counts_metadata_and_garbage() {
        assert_eq!(
            done_event(
                r#"{"prompt_tokens":7,"completion_tokens":3,"load_ms":120,"finish_reason":"stop","token_counts_estimated":true}"#
            ),
            MlxSwiftEvent::Done(MlxSwiftDone {
                prompt_tokens: Some(7),
                completion_tokens: Some(3),
                load_ms: Some(120),
                finish_reason: Some("stop".to_owned()),
                token_counts_estimated: true,
            })
        );
        assert_eq!(
            done_event(r#"{"prompt_tokens":null}"#),
            MlxSwiftEvent::Done(MlxSwiftDone::default())
        );
        assert_eq!(
            done_event("not json"),
            MlxSwiftEvent::Done(MlxSwiftDone::default())
        );
    }
}
