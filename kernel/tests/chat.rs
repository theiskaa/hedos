//! Tests for the chat-wire layer: role parsing, attachment rendering, the
//! lenient/strict message parsers, `parse_all`, tool-spec decoding, the outbound
//! `payload_value`, the inlined tool transcript, and the ChatML fallback.

use std::collections::BTreeMap;

use kernel::capabilities::ToolCall;
use kernel::capabilities::chat::{
    AttachmentKind, ChatAttachment, ChatMessage, ChatMlPrompt, ChatRole, ChatWireError,
    decode_tool_specs,
};
use kernel::records::JsonValue;

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn s(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn message(role: &str, content: &str) -> JsonValue {
    object([("role", s(role)), ("content", s(content))])
}

#[test]
fn roles_round_trip_through_the_wire() {
    for role in [
        ChatRole::System,
        ChatRole::User,
        ChatRole::Assistant,
        ChatRole::Tool,
    ] {
        assert_eq!(ChatRole::from_wire(role.as_str()), Some(role));
    }
    assert_eq!(ChatRole::from_wire("root"), None);
}

#[test]
fn a_document_attachment_inlines_as_a_wrapped_block() {
    let named = ChatAttachment {
        kind: AttachmentKind::Document,
        data: b"hello".to_vec(),
        mime_type: "text/plain".to_owned(),
        name: Some("notes.txt".to_owned()),
    };
    assert_eq!(
        named.inline_block().as_deref(),
        Some("<attached-file name=\"notes.txt\">\nhello\n</attached-file>")
    );

    let anonymous = ChatAttachment {
        name: None,
        ..named.clone()
    };
    assert_eq!(
        anonymous.inline_block().as_deref(),
        Some("<attached-file>\nhello\n</attached-file>")
    );
}

#[test]
fn an_image_attachment_has_no_inline_block() {
    let image = ChatAttachment {
        kind: AttachmentKind::Image,
        data: vec![1, 2, 3],
        mime_type: "image/png".to_owned(),
        name: None,
    };
    assert_eq!(image.inline_block(), None);
}

#[test]
fn payload_value_carries_role_and_content() {
    let payload = ChatMessage::new(ChatRole::User, "hi").payload_value();
    let fields = payload.as_object().expect("object");
    assert_eq!(fields.get("role"), Some(&s("user")));
    assert_eq!(fields.get("content"), Some(&s("hi")));
    assert!(fields.get("tool_calls").is_none());
    assert!(fields.get("images").is_none());
}

#[test]
fn payload_value_prepends_document_blocks_to_content() {
    let mut message = ChatMessage::new(ChatRole::User, "summarize this");
    message.attachments.push(ChatAttachment {
        kind: AttachmentKind::Document,
        data: b"file body".to_vec(),
        mime_type: "text/plain".to_owned(),
        name: None,
    });
    let fields = message.payload_value();
    let content = fields
        .as_object()
        .and_then(|o| o.get("content"))
        .and_then(JsonValue::as_str)
        .expect("content");
    assert_eq!(
        content,
        "<attached-file>\nfile body\n</attached-file>\n\nsummarize this"
    );
}

#[test]
fn payload_value_base64_encodes_images() {
    let mut message = ChatMessage::new(ChatRole::User, "look");
    message.attachments.push(ChatAttachment {
        kind: AttachmentKind::Image,
        data: b"Man".to_vec(),
        mime_type: "image/png".to_owned(),
        name: None,
    });
    let images = message
        .payload_value()
        .as_object()
        .and_then(|o| o.get("images"))
        .and_then(JsonValue::as_array)
        .map(<[_]>::to_vec)
        .expect("images");
    assert_eq!(images, vec![s("TWFu")]);
}

#[test]
fn payload_value_pads_short_base64() {
    let for_bytes = |bytes: &[u8]| {
        let mut message = ChatMessage::new(ChatRole::User, "");
        message.attachments.push(ChatAttachment {
            kind: AttachmentKind::Image,
            data: bytes.to_vec(),
            mime_type: "image/png".to_owned(),
            name: None,
        });
        message
            .payload_value()
            .as_object()
            .and_then(|o| o.get("images"))
            .and_then(JsonValue::as_array)
            .and_then(|images| images.first())
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .expect("image")
    };
    assert_eq!(for_bytes(b"Ma"), "TWE=");
    assert_eq!(for_bytes(b"M"), "TQ==");
}

#[test]
fn payload_value_carries_tool_routing() {
    let mut message = ChatMessage::new(ChatRole::Tool, "42");
    message.tool_call_id = Some("call_1".to_owned());
    message.tool_name = Some("add".to_owned());
    let fields = message.payload_value();
    let fields = fields.as_object().expect("object");
    assert_eq!(fields.get("tool_call_id"), Some(&s("call_1")));
    assert_eq!(fields.get("tool_name"), Some(&s("add")));
}

#[test]
fn from_payload_is_lenient() {
    let value = message("assistant", "sure");
    let parsed = ChatMessage::from_payload(&value).expect("parsed");
    assert_eq!(parsed.role, ChatRole::Assistant);
    assert_eq!(parsed.content, "sure");

    assert!(ChatMessage::from_payload(&object([("content", s("x"))])).is_none());
    assert!(ChatMessage::from_payload(&message("robot", "x")).is_none());

    // Absent content defaults to empty.
    let no_content = ChatMessage::from_payload(&object([("role", s("user"))])).expect("parsed");
    assert_eq!(no_content.content, "");
}

#[test]
fn from_payload_parses_tool_calls_and_drops_malformed() {
    let calls = JsonValue::Array(vec![
        object([
            ("id", s("call_1")),
            ("name", s("get")),
            ("arguments", object([])),
        ]),
        s("garbage"),
    ]);
    let value = object([
        ("role", s("assistant")),
        ("content", s("")),
        ("tool_calls", calls),
    ]);
    let parsed = ChatMessage::from_payload(&value).expect("parsed");
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].name, "get");
}

#[test]
fn parse_strict_rejects_bad_messages() {
    let not_object = ChatMessage::parse_strict(&s("x"), 2).expect_err("not object");
    assert!(matches!(not_object, ChatWireError::PayloadInvalid(m) if m.contains("index 2")));

    let bad_role = ChatMessage::parse_strict(&message("robot", "x"), 0).expect_err("bad role");
    assert!(matches!(bad_role, ChatWireError::PayloadInvalid(m) if m.contains("role")));

    let bad_content = ChatMessage::parse_strict(
        &object([("role", s("user")), ("content", JsonValue::Int(7))]),
        1,
    )
    .expect_err("bad content");
    assert!(matches!(bad_content, ChatWireError::PayloadInvalid(m) if m.contains("non-string")));
}

#[test]
fn parse_strict_treats_null_and_absent_content_as_empty() {
    let null = ChatMessage::parse_strict(
        &object([("role", s("user")), ("content", JsonValue::Null)]),
        0,
    )
    .expect("null content");
    assert_eq!(null.content, "");

    let absent = ChatMessage::parse_strict(&object([("role", s("user"))]), 0).expect("absent");
    assert_eq!(absent.content, "");
}

#[test]
fn parse_all_reads_a_messages_array() {
    let payload = object([(
        "messages",
        JsonValue::Array(vec![message("system", "be nice"), message("user", "hi")]),
    )]);
    let messages = ChatMessage::parse_all(&payload).expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, ChatRole::System);
    assert_eq!(messages[1].content, "hi");
}

#[test]
fn parse_all_falls_back_to_a_prompt_string() {
    let payload = object([("prompt", s("complete this"))]);
    let messages = ChatMessage::parse_all(&payload).expect("prompt");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, ChatRole::User);
    assert_eq!(messages[0].content, "complete this");
}

#[test]
fn parse_all_requires_messages_or_prompt() {
    let neither = ChatMessage::parse_all(&object([("model", s("x"))])).expect_err("neither");
    assert!(matches!(neither, ChatWireError::PayloadInvalid(_)));

    let not_object = ChatMessage::parse_all(&s("x")).expect_err("not object");
    assert!(matches!(not_object, ChatWireError::PayloadInvalid(_)));
}

#[test]
fn parse_all_propagates_a_strict_error_with_its_index() {
    let payload = object([(
        "messages",
        JsonValue::Array(vec![message("user", "ok"), object([("role", s("nope"))])]),
    )]);
    let error = ChatMessage::parse_all(&payload).expect_err("bad second message");
    assert!(matches!(error, ChatWireError::PayloadInvalid(m) if m.contains("index 1")));
}

#[test]
fn inlined_tool_transcript_folds_calls_into_text() {
    let mut message = ChatMessage::new(ChatRole::Assistant, "let me check");
    message.tool_calls.push(ToolCall::with_id(
        "call_1",
        "get_weather",
        object([("city", s("Paris"))]),
    ));
    let inlined = message.inlined_tool_transcript();
    assert_eq!(inlined.role, ChatRole::Assistant);
    assert!(inlined.tool_calls.is_empty());
    assert!(inlined.content.starts_with("let me check\n<tool_call>"));
    // Keys are sorted: arguments before name.
    assert!(
        inlined
            .content
            .contains("{\"arguments\":{\"city\":\"Paris\"},\"name\":\"get_weather\"}")
    );
}

#[test]
fn inlined_tool_transcript_leaves_other_messages_unchanged() {
    let user = ChatMessage::new(ChatRole::User, "hi");
    assert_eq!(user.inlined_tool_transcript(), user);

    let plain_assistant = ChatMessage::new(ChatRole::Assistant, "hello");
    assert_eq!(plain_assistant.inlined_tool_transcript(), plain_assistant);
}

#[test]
fn decode_tool_specs_reads_function_tools() {
    let tools = JsonValue::Array(vec![object([
        ("type", s("function")),
        (
            "function",
            object([
                ("name", s("get_weather")),
                ("description", s("look up weather")),
                ("parameters", object([("type", s("object"))])),
            ]),
        ),
    ])]);
    let specs = decode_tool_specs(Some(&tools)).expect("specs");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "get_weather");
    assert_eq!(specs[0].description, "look up weather");
    assert_eq!(specs[0].parameters, object([("type", s("object"))]));
}

#[test]
fn decode_tool_specs_defaults_type_and_parameters() {
    let tools = JsonValue::Array(vec![object([("function", object([("name", s("noop"))]))])]);
    let specs = decode_tool_specs(Some(&tools)).expect("specs");
    assert_eq!(specs[0].description, "");
    assert_eq!(specs[0].parameters, JsonValue::Object(BTreeMap::new()));
}

#[test]
fn decode_tool_specs_rejects_bad_shapes() {
    assert_eq!(decode_tool_specs(None).expect("absent"), Vec::new());

    let not_array = decode_tool_specs(Some(&s("x"))).expect_err("not array");
    assert!(matches!(not_array, ChatWireError::PayloadInvalid(_)));

    let no_name = JsonValue::Array(vec![object([("function", object([]))])]);
    assert!(decode_tool_specs(Some(&no_name)).is_err());

    let empty_name = JsonValue::Array(vec![object([("function", object([("name", s(""))]))])]);
    assert!(decode_tool_specs(Some(&empty_name)).is_err());

    let wrong_type = JsonValue::Array(vec![object([
        ("type", s("retrieval")),
        ("function", object([("name", s("x"))])),
    ])]);
    assert!(decode_tool_specs(Some(&wrong_type)).is_err());
}

#[test]
fn base64_covers_empty_and_multi_chunk_inputs() {
    let image_base64 = |bytes: &[u8]| {
        let mut message = ChatMessage::new(ChatRole::User, "");
        message.attachments.push(ChatAttachment {
            kind: AttachmentKind::Image,
            data: bytes.to_vec(),
            mime_type: "image/png".to_owned(),
            name: None,
        });
        message
            .payload_value()
            .as_object()
            .and_then(|o| o.get("images"))
            .and_then(JsonValue::as_array)
            .and_then(|images| images.first())
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
            .expect("image")
    };
    // Empty data still yields a (empty) base64 string in the images array.
    assert_eq!(image_base64(b""), "");
    // Multi-chunk (4/5/6 bytes) against known RFC 4648 vectors.
    assert_eq!(image_base64(b"Manu"), "TWFudQ==");
    assert_eq!(image_base64(b"Manua"), "TWFudWE=");
    assert_eq!(image_base64(b"Manual"), "TWFudWFs");
}

#[test]
fn decode_tool_specs_defaults_non_object_parameters() {
    for parameters in [
        s("nope"),
        JsonValue::Array(vec![JsonValue::Int(1)]),
        JsonValue::Null,
    ] {
        let tools = JsonValue::Array(vec![object([(
            "function",
            object([("name", s("f")), ("parameters", parameters)]),
        )])]);
        let specs = decode_tool_specs(Some(&tools)).expect("specs");
        assert_eq!(specs[0].parameters, JsonValue::Object(BTreeMap::new()));
    }
}

#[test]
fn decode_tool_specs_rejects_an_array_with_a_non_object_entry() {
    let tools = JsonValue::Array(vec![
        object([("function", object([("name", s("ok"))]))]),
        JsonValue::Int(42),
    ]);
    let error = decode_tool_specs(Some(&tools)).expect_err("non-object entry");
    assert!(
        matches!(error, ChatWireError::PayloadInvalid(m) if m.contains("array of function tools"))
    );
}

#[test]
fn from_payload_drops_a_call_with_non_object_arguments() {
    let calls = JsonValue::Array(vec![object([
        ("id", s("call_1")),
        ("name", s("get")),
        ("arguments", JsonValue::Int(5)),
    ])]);
    let value = object([("role", s("assistant")), ("tool_calls", calls)]);
    let parsed = ChatMessage::from_payload(&value).expect("parsed");
    assert!(parsed.tool_calls.is_empty());
}

#[test]
fn parse_all_ignores_non_array_messages_and_prefers_messages_over_prompt() {
    // A `messages` that is not an array falls through to `prompt`.
    let fallthrough = object([("messages", s("oops")), ("prompt", s("use me"))]);
    let messages = ChatMessage::parse_all(&fallthrough).expect("prompt fallback");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "use me");

    // With both a valid array and a prompt, the array wins.
    let both = object([
        ("messages", JsonValue::Array(vec![message("user", "array")])),
        ("prompt", s("ignored")),
    ]);
    let messages = ChatMessage::parse_all(&both).expect("array wins");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "array");
}

#[test]
fn inlined_tool_transcript_joins_multiple_calls_with_empty_content() {
    let mut message = ChatMessage::new(ChatRole::Assistant, "");
    message
        .tool_calls
        .push(ToolCall::with_id("c1", "one", object([])));
    message
        .tool_calls
        .push(ToolCall::with_id("c2", "two", object([])));
    let inlined = message.inlined_tool_transcript();
    // Empty content is filtered out; the two blocks join with a single newline.
    assert_eq!(
        inlined.content,
        "<tool_call>{\"arguments\":{},\"name\":\"one\"}</tool_call>\n\
         <tool_call>{\"arguments\":{},\"name\":\"two\"}</tool_call>"
    );
}

#[test]
fn chatml_render_of_no_messages_opens_only_the_assistant() {
    assert_eq!(ChatMlPrompt::render(&[]), "<|im_start|>assistant\n");
}

#[test]
fn chatml_render_wraps_each_turn_and_opens_the_assistant() {
    let messages = vec![
        ChatMessage::new(ChatRole::System, "be brief"),
        ChatMessage::new(ChatRole::User, "hi"),
    ];
    let prompt = ChatMlPrompt::render(&messages);
    assert_eq!(
        prompt,
        "<|im_start|>system\nbe brief<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n"
    );
    assert!(!ChatMlPrompt::NO_TEMPLATE_NOTICE.is_empty());
}
