//! The typed question a decision model answers (TypeSafe's System One):
//! `{"state": …, "questions": …, "images"?: […]}`, read the way llama-server
//! reads it.

use crate::records::JsonValue;

/// Whether the typed `question` holds an image, by the rule llama-server takes
/// one out with: a non-empty `images`, or an `image_url` part in a message's
/// content when `state` is a list of chat messages or an object holding one
/// under `messages`. A key named `image_url` anywhere else is part of the
/// state, not an image.
pub fn question_carries_images(question: &str) -> bool {
    let Ok(JsonValue::Object(fields)) = serde_json::from_str::<JsonValue>(question) else {
        return false;
    };
    let listed = fields
        .get("images")
        .and_then(JsonValue::as_array)
        .is_some_and(|images| !images.is_empty());
    let state = fields.get("state");
    let messages = match state.and_then(JsonValue::as_object) {
        Some(wrapped) => wrapped.get("messages"),
        None => state,
    };
    let in_messages = messages
        .and_then(JsonValue::as_array)
        .is_some_and(|messages| messages.iter().any(message_holds_image));
    listed || in_messages
}

fn message_holds_image(message: &JsonValue) -> bool {
    message
        .as_object()
        .and_then(|fields| fields.get("content"))
        .and_then(JsonValue::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.as_object().is_some_and(|fields| {
                    fields.get("type").and_then(JsonValue::as_str) == Some("image_url")
                        && fields.contains_key("image_url")
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PART: &str = r#"{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}"#;

    #[test]
    fn listed_images_are_images_and_an_empty_or_null_list_is_none() {
        let with = |images: &str| {
            question_carries_images(&format!(
                r#"{{"state":"s","questions":{{}},"images":{images}}}"#
            ))
        };
        assert!(with(r#"["data:image/png;base64,AA=="]"#));
        assert!(!with("[]"));
        assert!(!with("null"));
        assert!(!question_carries_images(r#"{"state":"s","questions":{}}"#));
    }

    #[test]
    fn an_image_part_of_a_message_state_is_an_image_bare_or_wrapped() {
        let bare =
            format!(r#"{{"state":[{{"role":"user","content":[{PART}]}}],"questions":{{}}}}"#);
        let wrapped = format!(
            r#"{{"state":{{"messages":[{{"role":"user","content":[{PART}]}}]}},"questions":{{}}}}"#
        );
        assert!(question_carries_images(&bare));
        assert!(question_carries_images(&wrapped));
    }

    #[test]
    fn a_state_that_only_names_an_image_url_holds_no_image() {
        for state in [
            r#"{"title":"Blue cotton shirt","image_url":"https://cdn.example.com/shirt.png"}"#,
            r#""see image_url in the brief""#,
            r#"[{"role":"user","content":"look at the image_url"}]"#,
            r#"[{"role":"user","content":[{"type":"text","text":"image_url"}]}]"#,
        ] {
            let question = format!(r#"{{"state":{state},"questions":{{}}}}"#);
            assert!(!question_carries_images(&question), "{state}");
        }
    }
}
