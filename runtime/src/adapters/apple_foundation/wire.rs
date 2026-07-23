//! The wire forms of the shim ABI: the request JSON `hedos_af_stream` takes
//! and the done-event payload it emits. Pure functions, so the FFI protocol
//! stays testable on every platform even though the FFI itself is compiled
//! only on macOS.

use kernel::capabilities::{ChatMessage, ToolCall, ToolSpec};
use kernel::records::JsonValue;
use serde_json::{Map, Value, json};

use super::backend::{BuiltinEvent, BuiltinOptions};

/// The request `hedos_af_stream` takes: the messages, the tools on offer, and
/// only the sampling options actually set. `seed` needs the full u64 range,
/// which is why this serializes through `serde_json` rather than the kernel's
/// i64-only [`kernel::records::JsonValue`].
pub(crate) fn request_json(
    messages: &[ChatMessage],
    tools: &[ToolSpec],
    options: &BuiltinOptions,
) -> String {
    let wire_messages: Vec<Value> = messages
        .iter()
        .map(|message| serde_value(&message.payload_value()))
        .collect();
    let mut object = Map::new();
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
    if let Some(top_k) = options.top_k {
        object.insert("top_k".to_owned(), json!(top_k));
    }
    if let Some(seed) = options.seed {
        object.insert("seed".to_owned(), json!(seed));
    }
    if let Some(max_tokens) = options.max_tokens {
        object.insert("max_tokens".to_owned(), json!(max_tokens));
    }
    Value::Object(object).to_string()
}

/// A kernel [`JsonValue`] as a `serde_json` value. Messages, tool specs, and
/// calls go through their canonical `payload_value` forms and then this
/// conversion, so the shim wire shape is pinned to those forms rather than to
/// whatever the kernel structs' derived serialization happens to be. The
/// fallback is unreachable — every `JsonValue` key is a string.
fn serde_value(value: &JsonValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Parse a tool-call-event payload (`{"id"?,"name","arguments"?:{…}}`) into
/// the event, or `None` for a payload without a usable call — a nameless or
/// malformed one is dropped, like any other unparseable frame on this wire.
/// An absent `arguments` defaults to `{}`, but a present non-object (null
/// included) drops the call — the shim must send an object or omit the key,
/// never null.
pub(crate) fn tool_call_event(payload: &str) -> Option<BuiltinEvent> {
    let value: JsonValue = serde_json::from_str(payload).ok()?;
    ToolCall::from_payload(&value)
        .filter(|call| !call.name.is_empty())
        .map(BuiltinEvent::ToolCall)
}

/// Parse a done-event payload (`{"prompt_tokens":n|null,"completion_tokens":
/// n|null}`) into the terminal event; malformed payloads become a done with
/// no counts rather than an error.
pub(crate) fn done_event(payload: &str) -> BuiltinEvent {
    let value: Option<Value> = serde_json::from_str(payload).ok();
    let count = |key: &str| {
        value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_i64)
    };
    BuiltinEvent::Done {
        prompt_tokens: count("prompt_tokens"),
        completion_tokens: count("completion_tokens"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::capabilities::ChatRole;

    #[test]
    fn the_request_carries_messages_and_only_the_set_options() {
        let messages = vec![
            ChatMessage::new(ChatRole::System, "be brief"),
            ChatMessage::new(ChatRole::User, "hi"),
        ];
        let options = BuiltinOptions {
            temperature: Some(0.5),
            top_k: Some(40),
            ..BuiltinOptions::default()
        };
        let parsed: Value = serde_json::from_str(&request_json(&messages, &[], &options)).unwrap();
        assert_eq!(parsed["messages"][0]["role"], "system");
        assert_eq!(parsed["messages"][1]["content"], "hi");
        assert_eq!(parsed["temperature"], 0.5);
        assert_eq!(parsed["top_k"], 40);
        assert!(parsed.get("tools").is_none());
        assert!(parsed.get("top_p").is_none());
        assert!(parsed.get("seed").is_none());
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
        let parsed: Value =
            serde_json::from_str(&request_json(&messages, &tools, &BuiltinOptions::default()))
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
        assert!(parsed["messages"][2].get("tool_calls").is_none());
        assert!(parsed["messages"][2].get("tool_call_id").is_none());
    }

    #[test]
    fn a_full_range_seed_survives_the_wire() {
        let options = BuiltinOptions {
            seed: Some(u64::MAX),
            ..BuiltinOptions::default()
        };
        let json = request_json(&[ChatMessage::new(ChatRole::User, "x")], &[], &options);
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["seed"].as_u64(), Some(u64::MAX));
    }

    #[test]
    fn tool_call_payloads_parse_mint_ids_and_drop_the_malformed() {
        let Some(BuiltinEvent::ToolCall(call)) =
            tool_call_event(r#"{"id":"call_9","name":"read","arguments":{"path":"a"}}"#)
        else {
            panic!("expected a tool call");
        };
        assert_eq!(call.id, "call_9");
        assert_eq!(call.name, "read");
        let Some(BuiltinEvent::ToolCall(minted)) = tool_call_event(r#"{"name":"read"}"#) else {
            panic!("expected a tool call with a minted id");
        };
        assert!(!minted.id.is_empty());
        assert_eq!(tool_call_event(r#"{"arguments":{}}"#), None);
        assert_eq!(tool_call_event(r#"{"name":""}"#), None);
        assert_eq!(tool_call_event(r#"{"name":"read","arguments":3}"#), None);
        assert_eq!(tool_call_event(r#"{"name":"read","arguments":null}"#), None);
        assert_eq!(tool_call_event("[]"), None);
        assert_eq!(tool_call_event("not json"), None);
    }

    #[test]
    fn a_non_object_parameters_spec_flows_through_verbatim() {
        // The gateway coerces malformed parameters to {} before dispatch, but
        // a direct dispatch caller can hand the adapter anything — the wire
        // forwards it as-is, so the shim's decode must tolerate the shape.
        let tools = vec![ToolSpec::new("odd", "", JsonValue::Int(3))];
        let json = request_json(
            &[ChatMessage::new(ChatRole::User, "x")],
            &tools,
            &BuiltinOptions::default(),
        );
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["tools"][0]["parameters"], 3);
    }

    #[test]
    fn done_payloads_parse_with_counts_nulls_or_garbage() {
        assert_eq!(
            done_event(r#"{"prompt_tokens":7,"completion_tokens":3}"#),
            BuiltinEvent::Done {
                prompt_tokens: Some(7),
                completion_tokens: Some(3)
            }
        );
        assert_eq!(
            done_event(r#"{"prompt_tokens":null,"completion_tokens":null}"#),
            BuiltinEvent::Done {
                prompt_tokens: None,
                completion_tokens: None
            }
        );
        assert_eq!(
            done_event("not json"),
            BuiltinEvent::Done {
                prompt_tokens: None,
                completion_tokens: None
            }
        );
    }
}
