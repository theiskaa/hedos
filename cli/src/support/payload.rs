//! Small builders for the `JsonValue` invoke payloads the commands construct.

use kernel::records::JsonValue;

/// A `{"role": …, "content": …}` chat message.
pub fn message(role: &str, content: &str) -> JsonValue {
    JsonValue::object([("role", role.into()), ("content", content.into())])
}
