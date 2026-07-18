//! Tests for the tool-calling wire types.

use kernel::capabilities::{CapabilityChunk, ToolCall, ToolSpec};
use kernel::records::JsonValue;

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

#[test]
fn tool_spec_round_trips_through_its_payload() {
    let params = object([("type", JsonValue::String("object".to_owned()))]);
    let spec = ToolSpec::new("get_weather", "Look up the weather", params.clone());
    let payload = spec.payload_value();
    assert_eq!(
        payload.as_object().and_then(|o| o.get("name")),
        Some(&JsonValue::String("get_weather".to_owned()))
    );

    let parsed = ToolSpec::from_payload(&payload).expect("parse");
    assert_eq!(parsed, spec);

    // A spec with no name doesn't parse; a missing description/params default.
    assert_eq!(ToolSpec::from_payload(&object([])), None);
    let bare = ToolSpec::from_payload(&object([("name", JsonValue::String("f".to_owned()))]))
        .expect("bare");
    assert_eq!(bare.description, "");
    assert_eq!(bare.parameters, object([]));
}

#[test]
fn tool_spec_from_payload_array_drops_bad_entries() {
    let value = JsonValue::Array(vec![
        object([("name", JsonValue::String("a".to_owned()))]),
        JsonValue::String("not a tool".to_owned()),
        object([("name", JsonValue::String("b".to_owned()))]),
    ]);
    let specs = ToolSpec::from_payload_array(Some(&value));
    assert_eq!(
        specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert!(ToolSpec::from_payload_array(None).is_empty());
}

#[test]
fn tool_call_generates_an_id_and_round_trips() {
    let args = object([("city", JsonValue::String("Paris".to_owned()))]);
    let call = ToolCall::new("get_weather", args.clone());
    assert!(call.id.starts_with("call_"), "generated id: {}", call.id);
    assert_eq!(call.name, "get_weather");

    let payload = call.payload_value();
    let parsed = ToolCall::from_payload(&payload).expect("parse");
    assert_eq!(parsed.id, call.id, "an explicit id round-trips");
    assert_eq!(parsed.arguments, args);
}

#[test]
fn tool_call_from_payload_rules() {
    // No name → None.
    assert_eq!(ToolCall::from_payload(&object([])), None);
    // Non-object arguments → None.
    assert_eq!(
        ToolCall::from_payload(&object([
            ("name", JsonValue::String("f".to_owned())),
            ("arguments", JsonValue::String("nope".to_owned())),
        ])),
        None
    );
    // Missing arguments default to {} and a fresh id is minted.
    let minted = ToolCall::from_payload(&object([("name", JsonValue::String("f".to_owned()))]))
        .expect("minted");
    assert!(minted.id.starts_with("call_"));
    assert_eq!(minted.arguments, object([]));
    // An empty id is treated as absent (fresh id minted).
    let empty_id = ToolCall::from_payload(&object([
        ("id", JsonValue::String(String::new())),
        ("name", JsonValue::String("f".to_owned())),
    ]))
    .expect("empty id");
    assert!(empty_id.id.starts_with("call_"));
}

#[test]
fn capability_chunk_carries_a_tool_call() {
    let call = ToolCall::with_id("call_1", "f", object([]));
    let chunk = CapabilityChunk::ToolCall(call.clone());
    assert_eq!(chunk, CapabilityChunk::ToolCall(call));
    assert_ne!(
        CapabilityChunk::ToolCall(ToolCall::with_id("call_1", "f", object([]))),
        CapabilityChunk::Text("f".to_owned())
    );
}
