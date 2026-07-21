//! Small builders for the `JsonValue` invoke payloads the commands construct.

use std::collections::BTreeMap;

use kernel::capabilities::{ChatAttachment, ChatMessage, ChatRole};
use kernel::records::JsonValue;

/// A `{"role": …, "content": …}` chat message.
pub fn message(role: &str, content: &str) -> JsonValue {
    JsonValue::object([("role", role.into()), ("content", content.into())])
}

/// A user chat message carrying image attachments, rendered to the kernel's
/// canonical wire form: the prompt as content plus a base64 `images` array, the
/// exact shape a vision model expects (and the gateway produces).
pub fn user_message_with_images(prompt: &str, images: Vec<ChatAttachment>) -> JsonValue {
    let mut message = ChatMessage::new(ChatRole::User, prompt);
    message.attachments = images;
    message.payload_value()
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

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::capabilities::AttachmentKind;

    fn image(data: &[u8]) -> ChatAttachment {
        ChatAttachment {
            kind: AttachmentKind::Image,
            data: data.to_vec(),
            mime_type: "image/png".to_owned(),
            name: None,
        }
    }

    #[test]
    fn a_user_message_renders_images_as_a_base64_array() {
        let value = user_message_with_images("what is this?", vec![image(b"hi")]);
        let object = value.as_object().expect("a message object");
        assert_eq!(object.get("role").and_then(JsonValue::as_str), Some("user"));
        assert_eq!(
            object.get("content").and_then(JsonValue::as_str),
            Some("what is this?")
        );
        let images = object
            .get("images")
            .and_then(JsonValue::as_array)
            .expect("an images array");
        assert_eq!(images.len(), 1);
        // base64 of "hi".
        assert_eq!(images[0].as_str(), Some("aGk="));
    }

    #[test]
    fn a_message_without_images_omits_the_key() {
        let value = user_message_with_images("hello", Vec::new());
        assert!(value.as_object().unwrap().get("images").is_none());
    }
}
