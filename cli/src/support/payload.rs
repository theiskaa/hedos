//! Small builders for the `JsonValue` invoke payloads the commands construct.

use std::collections::BTreeMap;

use kernel::records::JsonValue;

/// A `{"role": …, "content": …}` chat message.
pub fn message(role: &str, content: &str) -> JsonValue {
    JsonValue::object([("role", role.into()), ("content", content.into())])
}

/// The chat-payload core every command shares: a `messages` array plus an
/// optional token cap. Returned as the open map so a caller can add its own
/// knobs (a temperature, a tools probe) before wrapping it in
/// [`JsonValue::Object`].
pub fn chat(messages: Vec<JsonValue>, max_tokens: Option<i64>) -> BTreeMap<String, JsonValue> {
    let mut payload = BTreeMap::new();
    payload.insert("messages".to_owned(), JsonValue::Array(messages));
    if let Some(max_tokens) = max_tokens {
        payload.insert("max_tokens".to_owned(), JsonValue::Int(max_tokens));
    }
    payload
}
