//! Decoding an Ollama `/api/chat` request body into the kernel's chat model: the
//! messages (with inline base64 images), the `options` block mapped onto the
//! kernel's parameter names, the `think`/`format` directives, and tool calls.

use std::collections::BTreeMap;

use kernel::capabilities::{
    AttachmentKind, ChatAttachment, ChatMessage, ChatRole, ToolCall, ToolSpec, decode_tool_specs,
};
use kernel::records::JsonValue;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::wire::{base64, param_decoding};

/// `i64::MIN`/`i64::MAX` as floats, for range-checking integer-valued floats.
/// `i64::MAX as f64` rounds up to 2^63, one past the real max, so the upper
/// bound is exclusive.
const MIN_I64_AS_F64: f64 = i64::MIN as f64;
const MAX_I64_AS_F64: f64 = i64::MAX as f64;

/// A decoded Ollama chat request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// The requested model id or alias.
    pub model: String,
    /// The conversation so far.
    pub messages: Vec<ChatMessage>,
    /// Whether to stream (Ollama defaults this to `true`).
    pub stream: bool,
    /// The generation options, keyed by the kernel's parameter names.
    pub options: BTreeMap<String, JsonValue>,
    /// The function tools the model may call.
    pub tools: Vec<ToolSpec>,
}

/// The top-level request keys the chat surface honors.
const HONORED_KEYS: &[&str] = &[
    "model",
    "messages",
    "stream",
    "think",
    "options",
    "format",
    "tools",
    "keep_alive",
];

/// Map an Ollama option name to the kernel parameter name it becomes, or `None`
/// if it isn't a supported option.
fn option_payload(key: &str) -> Option<&'static str> {
    match key {
        "temperature" => Some("temperature"),
        "top_p" => Some("top_p"),
        "top_k" => Some("top_k"),
        "min_p" => Some("min_p"),
        "num_predict" => Some("max_tokens"),
        "num_ctx" => Some("context_length"),
        "seed" => Some("seed"),
        "repeat_penalty" => Some("repeat_penalty"),
        "frequency_penalty" => Some("frequency_penalty"),
        "presence_penalty" => Some("presence_penalty"),
        _ => None,
    }
}

fn bad_request(message: impl Into<String>) -> GatewayError {
    GatewayError::new(GatewayErrorKind::BadRequest, message)
}

/// Decode an Ollama chat request from its JSON object body.
pub fn decode_chat_request(
    body: &BTreeMap<String, JsonValue>,
) -> Result<ChatRequest, GatewayError> {
    let model = body
        .get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))?
        .to_owned();

    let raw_messages = param_decoding::object_array(body.get("messages"))
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| bad_request("messages is required"))?;

    param_decoding::reject_unknown_keys(body, HONORED_KEYS, "parameter")?;

    let messages = raw_messages
        .iter()
        .map(|fields| decode_message(fields))
        .collect::<Result<Vec<_>, _>>()?;

    let options = decode_options(body)?;
    let tools =
        decode_tool_specs(body.get("tools")).map_err(|error| bad_request(error.to_string()))?;

    Ok(ChatRequest {
        model,
        messages,
        // Ollama streams by default.
        stream: body
            .get("stream")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true),
        options,
        tools,
    })
}

fn decode_message(raw: &BTreeMap<String, JsonValue>) -> Result<ChatMessage, GatewayError> {
    let raw_role = raw.get("role").and_then(JsonValue::as_str).unwrap_or("");
    let role = ChatRole::from_wire(raw_role)
        .ok_or_else(|| bad_request(format!("unsupported message role {raw_role}")))?;

    let mut attachments = Vec::new();
    if let Some(JsonValue::Array(images)) = raw.get("images")
        && !images.is_empty()
    {
        for image in images {
            let data = image
                .as_str()
                .and_then(base64::decode)
                .ok_or_else(|| bad_request("each image must be a base64-encoded string"))?;
            attachments.push(ChatAttachment {
                kind: AttachmentKind::Image,
                data,
                mime_type: "image/png".to_owned(),
                name: None,
            });
        }
    }

    let tool_calls = decode_tool_calls(raw.get("tool_calls"))?;
    let content = raw.get("content").and_then(JsonValue::as_str);

    if role == ChatRole::Tool {
        let mut message = ChatMessage::new(ChatRole::Tool, content.unwrap_or(""));
        message.tool_call_id = raw
            .get("tool_call_id")
            .and_then(JsonValue::as_str)
            .map(str::to_owned);
        message.tool_name = raw
            .get("tool_name")
            .and_then(JsonValue::as_str)
            .map(str::to_owned);
        return Ok(message);
    }

    if role == ChatRole::Assistant && !tool_calls.is_empty() {
        let mut message = ChatMessage::new(ChatRole::Assistant, content.unwrap_or(""));
        message.tool_calls = tool_calls;
        return Ok(message);
    }

    let content = content.ok_or_else(|| bad_request("message content must be a string"))?;
    let mut message = ChatMessage::new(role, content);
    message.attachments = attachments;
    Ok(message)
}

fn decode_options(
    body: &BTreeMap<String, JsonValue>,
) -> Result<BTreeMap<String, JsonValue>, GatewayError> {
    let mut options = BTreeMap::new();
    if let Some(JsonValue::Object(raw_options)) = body.get("options") {
        // BTreeMap iterates in sorted key order, matching Swift's `keys.sorted()`.
        for (key, value) in raw_options {
            if key == "stop" {
                if let Some(stop) = param_decoding::stop(Some(value), None)? {
                    options.insert("stop".to_owned(), stop);
                }
                continue;
            }
            let payload = option_payload(key)
                .ok_or_else(|| {
                    bad_request(format!("the option '{key}' is not supported"))
                        .with_code("unsupported_parameter")
                })?
                .to_owned();
            options.insert(payload, option_number(key, value)?);
        }
    }
    if let Some(think) = body.get("think").and_then(JsonValue::as_bool) {
        options.insert("thinking".to_owned(), JsonValue::Bool(think));
    }
    if let Some(format) = decode_format(body.get("format"))? {
        options.insert("response_format".to_owned(), format);
    }
    Ok(options)
}

/// An option value as a number: an integer, or an integer-valued float as an
/// integer (Swift tries `as? Int` first), else a float. Anything else is a bad
/// request.
fn option_number(key: &str, value: &JsonValue) -> Result<JsonValue, GatewayError> {
    match value {
        JsonValue::Int(number) => Ok(JsonValue::Int(*number)),
        JsonValue::Double(number)
            if number.fract() == 0.0 && *number >= MIN_I64_AS_F64 && *number < MAX_I64_AS_F64 =>
        {
            Ok(JsonValue::Int(*number as i64))
        }
        JsonValue::Double(number) => Ok(JsonValue::Double(*number)),
        _ => Err(bad_request(format!("the option '{key}' must be a number"))
            .with_code("unsupported_parameter")),
    }
}

fn decode_format(raw: Option<&JsonValue>) -> Result<Option<JsonValue>, GatewayError> {
    let invalid = || bad_request("format must be \"json\" or a JSON schema object");
    match raw {
        None => Ok(None),
        Some(JsonValue::String(text)) => {
            if text != "json" {
                return Err(invalid());
            }
            Ok(Some(object([(
                "type",
                JsonValue::String("json_object".to_owned()),
            )])))
        }
        Some(schema @ JsonValue::Object(_)) => Ok(Some(object([
            ("type", JsonValue::String("json_schema".to_owned())),
            ("json_schema", object([("schema", schema.clone())])),
        ]))),
        Some(_) => Err(invalid()),
    }
}

fn decode_tool_calls(raw: Option<&JsonValue>) -> Result<Vec<ToolCall>, GatewayError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Some(entries) = param_decoding::object_array(Some(raw)) else {
        return Err(bad_request("tool_calls must be an array"));
    };
    let mut calls = Vec::with_capacity(entries.len());
    for entry in entries {
        let function = entry.get("function").and_then(JsonValue::as_object);
        let name = function
            .and_then(|function| function.get("name"))
            .and_then(JsonValue::as_str)
            .filter(|name| !name.is_empty());
        let (Some(function), Some(name)) = (function, name) else {
            return Err(bad_request("each tool call must carry function.name"));
        };
        // Ollama arguments are an inline object; a missing or non-object value
        // defaults to an empty object rather than erroring.
        let arguments = match function.get("arguments") {
            Some(object @ JsonValue::Object(_)) => object.clone(),
            _ => JsonValue::Object(BTreeMap::new()),
        };
        // A present string id is honored verbatim (Swift checks only presence).
        let id = entry.get("id").and_then(JsonValue::as_str);
        calls.push(match id {
            Some(id) => ToolCall::with_id(id, name, arguments),
            None => ToolCall::new(name, arguments),
        });
    }
    Ok(calls)
}

/// The kernel dispatch payload for a decoded request: the messages and tools,
/// with the options merged in at the top level.
pub fn chat_payload(request: &ChatRequest) -> JsonValue {
    let mut payload: BTreeMap<String, JsonValue> = BTreeMap::new();
    payload.insert(
        "messages".to_owned(),
        JsonValue::Array(
            request
                .messages
                .iter()
                .map(ChatMessage::payload_value)
                .collect(),
        ),
    );
    if !request.tools.is_empty() {
        payload.insert(
            "tools".to_owned(),
            JsonValue::Array(request.tools.iter().map(ToolSpec::payload_value).collect()),
        );
    }
    for (key, value) in &request.options {
        payload.insert(key.clone(), value.clone());
    }
    JsonValue::Object(payload)
}

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn string(value: &str) -> JsonValue {
        JsonValue::String(value.to_owned())
    }

    fn message(role: &str, content: &str) -> JsonValue {
        JsonValue::Object(map(&[("role", string(role)), ("content", string(content))]))
    }

    fn minimal(extra: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        let mut body = map(&[
            ("model", string("llama3")),
            ("messages", JsonValue::Array(vec![message("user", "hi")])),
        ]);
        for (key, value) in extra {
            body.insert(key.to_string(), value.clone());
        }
        body
    }

    #[test]
    fn a_minimal_request_defaults_stream_to_true() {
        let request = decode_chat_request(&minimal(&[])).unwrap();
        assert_eq!(request.model, "llama3");
        assert_eq!(request.messages.len(), 1);
        // Ollama streams by default.
        assert!(request.stream);
    }

    #[test]
    fn stream_false_is_honored() {
        let request = decode_chat_request(&minimal(&[("stream", JsonValue::Bool(false))])).unwrap();
        assert!(!request.stream);
    }

    #[test]
    fn a_missing_model_or_messages_is_rejected() {
        let mut body = minimal(&[]);
        body.remove("model");
        assert!(decode_chat_request(&body).is_err());
        let mut body = minimal(&[]);
        body.insert("messages".to_owned(), JsonValue::Array(Vec::new()));
        assert!(decode_chat_request(&body).is_err());
    }

    #[test]
    fn an_unknown_top_level_key_is_rejected() {
        let body = minimal(&[("frobnicate", JsonValue::Bool(true))]);
        assert!(decode_chat_request(&body).is_err());
    }

    #[test]
    fn options_are_mapped_to_kernel_names() {
        let options = JsonValue::Object(map(&[
            ("num_predict", JsonValue::Int(256)),
            ("num_ctx", JsonValue::Int(4096)),
            ("temperature", JsonValue::Double(0.7)),
        ]));
        let request = decode_chat_request(&minimal(&[("options", options)])).unwrap();
        assert_eq!(
            request.options.get("max_tokens"),
            Some(&JsonValue::Int(256))
        );
        assert_eq!(
            request.options.get("context_length"),
            Some(&JsonValue::Int(4096))
        );
        assert_eq!(
            request.options.get("temperature"),
            Some(&JsonValue::Double(0.7))
        );
    }

    #[test]
    fn an_integer_valued_float_option_becomes_an_int() {
        let options = JsonValue::Object(map(&[("seed", JsonValue::Double(7.0))]));
        let request = decode_chat_request(&minimal(&[("options", options)])).unwrap();
        assert_eq!(request.options.get("seed"), Some(&JsonValue::Int(7)));
    }

    #[test]
    fn an_out_of_range_integer_valued_float_option_stays_a_double() {
        // 1e19 overflows i64; Swift's `as? Int` fails and falls to Double, so it
        // must not be saturated to i64::MAX.
        let options = JsonValue::Object(map(&[("seed", JsonValue::Double(1e19))]));
        let request = decode_chat_request(&minimal(&[("options", options)])).unwrap();
        assert_eq!(request.options.get("seed"), Some(&JsonValue::Double(1e19)));
    }

    #[test]
    fn an_unknown_option_or_non_number_is_rejected() {
        let options = JsonValue::Object(map(&[("mirostat", JsonValue::Int(1))]));
        assert!(decode_chat_request(&minimal(&[("options", options)])).is_err());
        let options = JsonValue::Object(map(&[("temperature", string("hot"))]));
        assert!(decode_chat_request(&minimal(&[("options", options)])).is_err());
    }

    #[test]
    fn the_stop_option_is_normalized() {
        let options = JsonValue::Object(map(&[("stop", string("END"))]));
        let request = decode_chat_request(&minimal(&[("options", options)])).unwrap();
        assert_eq!(
            request.options.get("stop"),
            Some(&JsonValue::Array(vec![string("END")]))
        );
    }

    #[test]
    fn think_becomes_a_thinking_option() {
        let request = decode_chat_request(&minimal(&[("think", JsonValue::Bool(true))])).unwrap();
        assert_eq!(
            request.options.get("thinking"),
            Some(&JsonValue::Bool(true))
        );
    }

    #[test]
    fn format_json_string_becomes_a_json_object_schema() {
        let request = decode_chat_request(&minimal(&[("format", string("json"))])).unwrap();
        assert_eq!(
            request.options.get("response_format"),
            Some(&object([("type", string("json_object"))]))
        );
    }

    #[test]
    fn format_object_becomes_a_json_schema() {
        let schema = JsonValue::Object(map(&[("type", string("object"))]));
        let request = decode_chat_request(&minimal(&[("format", schema.clone())])).unwrap();
        let expected = object([
            ("type", string("json_schema")),
            ("json_schema", object([("schema", schema)])),
        ]);
        assert_eq!(request.options.get("response_format"), Some(&expected));
    }

    #[test]
    fn a_bad_format_string_is_rejected() {
        assert!(decode_chat_request(&minimal(&[("format", string("yaml"))])).is_err());
    }

    #[test]
    fn images_decode_into_attachments() {
        let images = JsonValue::Array(vec![string("aGk=")]); // "hi"
        let msg = JsonValue::Object(map(&[
            ("role", string("user")),
            ("content", string("look")),
            ("images", images),
        ]));
        let request =
            decode_chat_request(&minimal(&[("messages", JsonValue::Array(vec![msg]))])).unwrap();
        assert_eq!(request.messages[0].attachments.len(), 1);
        assert_eq!(request.messages[0].attachments[0].data, b"hi");
        assert_eq!(request.messages[0].attachments[0].mime_type, "image/png");
    }

    #[test]
    fn a_non_base64_image_is_rejected() {
        let images = JsonValue::Array(vec![string("!!!not base64!!!")]);
        let msg = JsonValue::Object(map(&[("role", string("user")), ("images", images)]));
        assert!(
            decode_chat_request(&minimal(&[("messages", JsonValue::Array(vec![msg]))])).is_err()
        );
    }

    #[test]
    fn a_tool_role_message_routes_by_name() {
        let msg = JsonValue::Object(map(&[
            ("role", string("tool")),
            ("content", string("42")),
            ("tool_name", string("adder")),
        ]));
        let request =
            decode_chat_request(&minimal(&[("messages", JsonValue::Array(vec![msg]))])).unwrap();
        assert_eq!(request.messages[0].role, ChatRole::Tool);
        assert_eq!(request.messages[0].tool_name.as_deref(), Some("adder"));
    }

    #[test]
    fn a_non_string_content_is_rejected_for_a_plain_message() {
        let msg = JsonValue::Object(map(&[
            ("role", string("user")),
            ("content", JsonValue::Int(5)),
        ]));
        assert!(
            decode_chat_request(&minimal(&[("messages", JsonValue::Array(vec![msg]))])).is_err()
        );
    }

    #[test]
    fn chat_payload_merges_messages_tools_and_options() {
        let options = JsonValue::Object(map(&[("temperature", JsonValue::Double(0.5))]));
        let request = decode_chat_request(&minimal(&[("options", options)])).unwrap();
        let JsonValue::Object(payload) = chat_payload(&request) else {
            panic!("expected object");
        };
        assert!(payload.contains_key("messages"));
        assert_eq!(payload.get("temperature"), Some(&JsonValue::Double(0.5)));
        assert!(!payload.contains_key("tools"));
    }
}
