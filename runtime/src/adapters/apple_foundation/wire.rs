//! The wire forms of the shim ABI: the request JSON `hedos_af_stream` takes
//! and the done-event payload it emits. Pure functions, so the FFI protocol
//! stays testable on every platform even though the FFI itself is
//! feature-gated to macOS.

use kernel::capabilities::ChatMessage;
use serde_json::{Map, Value, json};

use super::backend::{BuiltinEvent, BuiltinOptions};

/// The request `hedos_af_stream` takes: the messages plus only the sampling
/// options actually set. `seed` needs the full u64 range, which is why this
/// serializes through `serde_json` rather than the kernel's i64-only
/// [`kernel::records::JsonValue`].
pub(crate) fn request_json(messages: &[ChatMessage], options: &BuiltinOptions) -> String {
    let wire_messages: Vec<Value> = messages
        .iter()
        .map(|message| json!({"role": message.role.as_str(), "content": message.content}))
        .collect();
    let mut object = Map::new();
    object.insert("messages".to_owned(), Value::Array(wire_messages));
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
        let parsed: Value = serde_json::from_str(&request_json(&messages, &options)).unwrap();
        assert_eq!(parsed["messages"][0]["role"], "system");
        assert_eq!(parsed["messages"][1]["content"], "hi");
        assert_eq!(parsed["temperature"], 0.5);
        assert_eq!(parsed["top_k"], 40);
        assert!(parsed.get("top_p").is_none());
        assert!(parsed.get("seed").is_none());
        assert!(parsed.get("max_tokens").is_none());
    }

    #[test]
    fn a_full_range_seed_survives_the_wire() {
        let options = BuiltinOptions {
            seed: Some(u64::MAX),
            ..BuiltinOptions::default()
        };
        let json = request_json(&[ChatMessage::new(ChatRole::User, "x")], &options);
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["seed"].as_u64(), Some(u64::MAX));
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
