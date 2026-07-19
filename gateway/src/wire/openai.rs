//! Decoding an OpenAI `/v1/chat/completions` request body into the kernel's chat
//! model: the message list, tool specs and calls, image `data:` URIs, and the
//! honored sampling parameters, rejecting anything the gateway doesn't support.

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

/// A decoded chat-completions request.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// The requested model id or alias.
    pub model: String,
    /// The conversation so far.
    pub messages: Vec<ChatMessage>,
    /// Whether the response should stream as SSE.
    pub stream: bool,
    /// Whether a streamed response should include a final usage frame.
    pub include_usage: bool,
    /// The honored sampling parameters, keyed by wire name.
    pub sampling: BTreeMap<String, JsonValue>,
    /// The function tools the model may call.
    pub tools: Vec<ToolSpec>,
    /// The `tool_choice` directive, passed through verbatim.
    pub tool_choice: Option<JsonValue>,
}

/// The request keys the chat surface honors; any other key is rejected.
const HONORED_KEYS: &[&str] = &[
    "model",
    "messages",
    "stream",
    "stream_options",
    "temperature",
    "top_p",
    "max_tokens",
    "max_completion_tokens",
    "stop",
    "seed",
    "n",
    "frequency_penalty",
    "presence_penalty",
    "response_format",
    "tools",
    "tool_choice",
    "user",
];

fn bad_request(message: impl Into<String>) -> GatewayError {
    GatewayError::new(GatewayErrorKind::BadRequest, message)
}

/// Decode a chat-completions request from its JSON object body. Takes the body
/// by value: the caller has finished with it, and the request may carry large
/// base64 image data that should not be cloned.
pub fn decode_chat_request(
    mut body: BTreeMap<String, JsonValue>,
) -> Result<ChatRequest, GatewayError> {
    // An empty logit_bias is tolerated (and ignored) rather than rejected as an
    // unhonored key.
    if let Some(JsonValue::Object(bias)) = body.get("logit_bias")
        && bias.is_empty()
    {
        body.remove("logit_bias");
    }

    let model = body
        .get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))?
        .to_owned();

    let raw_messages = message_objects(body.get("messages"))
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| bad_request("messages is required"))?;

    param_decoding::reject_unknown_keys(&body, HONORED_KEYS, "parameter")?;

    let messages = raw_messages
        .iter()
        .map(|fields| decode_message(fields))
        .collect::<Result<Vec<_>, _>>()?;

    let mut sampling = decode_sampling(&body)?;

    let include_usage = body
        .get("stream_options")
        .and_then(JsonValue::as_object)
        .and_then(|options| options.get("include_usage"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);

    if let Some(response_format) = decode_response_format(body.get("response_format"))? {
        sampling.insert("response_format".to_owned(), response_format);
    }

    let tools =
        decode_tool_specs(body.get("tools")).map_err(|error| bad_request(error.to_string()))?;

    let tool_choice = match body.get("tool_choice") {
        None => None,
        Some(choice @ (JsonValue::String(_) | JsonValue::Object(_))) => Some(choice.clone()),
        Some(_) => return Err(bad_request("tool_choice must be a string or object")),
    };

    Ok(ChatRequest {
        model,
        messages,
        stream: body
            .get("stream")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        include_usage,
        sampling,
        tools,
        tool_choice,
    })
}

/// The `messages` value as a slice of objects, or `None` if it isn't an array of
/// objects (matching Swift's `as? [[String: Any]]` all-or-nothing cast).
fn message_objects(value: Option<&JsonValue>) -> Option<Vec<&BTreeMap<String, JsonValue>>> {
    let JsonValue::Array(items) = value? else {
        return None;
    };
    items.iter().map(JsonValue::as_object).collect()
}

/// A numeric parameter as a float. Accepts an integer or a float, but — like
/// Swift's exact `as? Double` bridging — rejects an integer too large to be
/// represented in a float without loss.
fn number_param(
    body: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<Option<f64>, GatewayError> {
    let invalid = || bad_request(format!("{key} must be a number")).with_code("invalid_type");
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::Int(value)) => {
            let as_float = *value as f64;
            if as_float as i64 == *value {
                Ok(Some(as_float))
            } else {
                Err(invalid())
            }
        }
        Some(JsonValue::Double(value)) => Ok(Some(*value)),
        Some(_) => Err(invalid()),
    }
}

/// An integer parameter. Accepts an integer or an integer-valued, in-range float
/// (as Swift's `as? Int` does); a fractional or out-of-range float is rejected.
fn int_param(body: &BTreeMap<String, JsonValue>, key: &str) -> Result<Option<i64>, GatewayError> {
    match body.get(key) {
        None => Ok(None),
        Some(JsonValue::Int(value)) => Ok(Some(*value)),
        Some(JsonValue::Double(value))
            if value.fract() == 0.0 && *value >= MIN_I64_AS_F64 && *value < MAX_I64_AS_F64 =>
        {
            Ok(Some(*value as i64))
        }
        Some(_) => Err(bad_request(format!("{key} must be an integer")).with_code("invalid_type")),
    }
}

fn decode_sampling(
    body: &BTreeMap<String, JsonValue>,
) -> Result<BTreeMap<String, JsonValue>, GatewayError> {
    let mut sampling = BTreeMap::new();
    if let Some(temperature) = number_param(body, "temperature")? {
        sampling.insert("temperature".to_owned(), JsonValue::Double(temperature));
    }
    if let Some(top_p) = number_param(body, "top_p")? {
        sampling.insert("top_p".to_owned(), JsonValue::Double(top_p));
    }
    if let Some(max_tokens) = int_param(body, "max_tokens")? {
        sampling.insert("max_tokens".to_owned(), JsonValue::Int(max_tokens));
    } else if let Some(max_tokens) = int_param(body, "max_completion_tokens")? {
        sampling.insert("max_tokens".to_owned(), JsonValue::Int(max_tokens));
    }
    if let Some(stop) = param_decoding::stop(body.get("stop"), Some(4))? {
        sampling.insert("stop".to_owned(), stop);
    }
    if let Some(seed) = int_param(body, "seed")? {
        sampling.insert("seed".to_owned(), JsonValue::Int(seed));
    }
    if let Some(n) = int_param(body, "n")?
        && n != 1
    {
        return Err(
            bad_request("n greater than 1 is not supported").with_code("unsupported_parameter")
        );
    }
    if let Some(frequency_penalty) = number_param(body, "frequency_penalty")? {
        sampling.insert(
            "frequency_penalty".to_owned(),
            JsonValue::Double(frequency_penalty),
        );
    }
    if let Some(presence_penalty) = number_param(body, "presence_penalty")? {
        sampling.insert(
            "presence_penalty".to_owned(),
            JsonValue::Double(presence_penalty),
        );
    }
    Ok(sampling)
}

fn decode_response_format(raw: Option<&JsonValue>) -> Result<Option<JsonValue>, GatewayError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let object = raw
        .as_object()
        .ok_or_else(|| bad_request("response_format must be an object with a type"))?;
    let format_type = object
        .get("type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| bad_request("response_format must be an object with a type"))?;
    match format_type {
        "text" => Ok(None),
        "json_object" | "json_schema" => Ok(Some(raw.clone())),
        other => Err(
            bad_request(format!("response_format type '{other}' is not supported"))
                .with_code("unsupported_parameter"),
        ),
    }
}

fn decode_message(raw: &BTreeMap<String, JsonValue>) -> Result<ChatMessage, GatewayError> {
    let raw_role = raw.get("role").and_then(JsonValue::as_str).unwrap_or("");
    let role = match raw_role {
        "system" | "developer" => ChatRole::System,
        "user" => ChatRole::User,
        "assistant" => ChatRole::Assistant,
        "tool" => ChatRole::Tool,
        other => return Err(bad_request(format!("unsupported message role {other}"))),
    };

    if role == ChatRole::Tool {
        let call_id = raw
            .get("tool_call_id")
            .and_then(JsonValue::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| bad_request("tool messages require tool_call_id"))?;
        let content = raw.get("content").and_then(JsonValue::as_str).unwrap_or("");
        let mut message = ChatMessage::new(ChatRole::Tool, content);
        message.tool_call_id = Some(call_id.to_owned());
        message.tool_name = raw
            .get("name")
            .and_then(JsonValue::as_str)
            .map(str::to_owned);
        return Ok(message);
    }

    let tool_calls = decode_tool_calls(raw.get("tool_calls"))?;

    if let Some(content) = raw.get("content").and_then(JsonValue::as_str) {
        let mut message = ChatMessage::new(role, content);
        message.tool_calls = tool_calls;
        return Ok(message);
    }

    if let Some(parts) = content_parts(raw.get("content")) {
        let mut texts = String::new();
        let mut attachments = Vec::new();
        for part in parts {
            match part.get("type").and_then(JsonValue::as_str) {
                Some("text") => {
                    let text = part
                        .get("text")
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| bad_request("text content part is missing its text"))?;
                    texts.push_str(text);
                }
                Some("image_url") => {
                    let url = part
                        .get("image_url")
                        .and_then(JsonValue::as_object)
                        .and_then(|field| field.get("url"))
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| bad_request("image_url content part is missing its url"))?;
                    attachments.push(decode_image_data_uri(url)?);
                }
                _ => {
                    return Err(bad_request(
                        "only text and image_url content parts are supported",
                    ));
                }
            }
        }
        let mut message = ChatMessage::new(role, texts);
        message.tool_calls = tool_calls;
        message.attachments = attachments;
        return Ok(message);
    }

    if role == ChatRole::Assistant && !tool_calls.is_empty() {
        let mut message = ChatMessage::new(ChatRole::Assistant, "");
        message.tool_calls = tool_calls;
        return Ok(message);
    }

    Err(bad_request(
        "message content must be a string or text parts",
    ))
}

/// The content value as a slice of part objects, or `None` if it isn't an array
/// of objects (Swift's `as? [[String: Any]]`).
fn content_parts(value: Option<&JsonValue>) -> Option<Vec<&BTreeMap<String, JsonValue>>> {
    let JsonValue::Array(items) = value? else {
        return None;
    };
    items.iter().map(JsonValue::as_object).collect()
}

/// Decode an image `data:` URI into an attachment. Only `data:` URIs are
/// accepted — the gateway never fetches anything off the machine.
pub(crate) fn decode_image_data_uri(url: &str) -> Result<ChatAttachment, GatewayError> {
    let Some(body) = url.strip_prefix("data:") else {
        return Err(bad_request(
            "image urls must be data: URIs — this gateway fetches nothing off this machine",
        ));
    };
    let Some(comma) = body.find(',') else {
        return Err(bad_request("malformed image data: URI"));
    };
    let meta = &body[..comma];
    let encoded = &body[comma + 1..];
    let data = match base64::decode(encoded) {
        Some(data) if meta.contains("base64") => data,
        _ => return Err(bad_request("image data: URI must carry base64 content")),
    };
    let mediatype = meta.split(';').next().unwrap_or("");
    let mime_type = if mediatype.is_empty() || mediatype == "base64" {
        "image/png".to_owned()
    } else {
        mediatype.to_owned()
    };
    Ok(ChatAttachment {
        kind: AttachmentKind::Image,
        data,
        mime_type,
        name: None,
    })
}

fn decode_tool_calls(raw: Option<&JsonValue>) -> Result<Vec<ToolCall>, GatewayError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Some(entries) = call_objects(raw) else {
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
        let arguments = decode_tool_arguments(function.get("arguments"))?;
        // A present string id is honored verbatim, even if empty (Swift checks
        // only for presence here); an absent one mints a fresh call id.
        let id = entry.get("id").and_then(JsonValue::as_str);
        calls.push(match id {
            Some(id) => ToolCall::with_id(id, name, arguments),
            None => ToolCall::new(name, arguments),
        });
    }
    Ok(calls)
}

/// A tool call's `arguments`: a JSON-encoded object string (rejected if it
/// doesn't parse to an object), an inline object, or — when absent or any other
/// type — an empty object (matching Swift's lenient fallback).
fn decode_tool_arguments(raw: Option<&JsonValue>) -> Result<JsonValue, GatewayError> {
    match raw {
        Some(JsonValue::String(encoded)) => match serde_json::from_str::<JsonValue>(encoded) {
            Ok(value @ JsonValue::Object(_)) => Ok(value),
            _ => Err(bad_request(
                "tool call arguments must be a JSON-encoded object",
            )),
        },
        Some(object @ JsonValue::Object(_)) => Ok(object.clone()),
        _ => Ok(JsonValue::Object(BTreeMap::new())),
    }
}

/// The `tool_calls` value as a slice of objects, or `None` if it isn't an array
/// of objects.
fn call_objects(value: &JsonValue) -> Option<Vec<&BTreeMap<String, JsonValue>>> {
    let JsonValue::Array(items) = value else {
        return None;
    };
    items.iter().map(JsonValue::as_object).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(pairs: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn string(value: &str) -> JsonValue {
        JsonValue::String(value.to_owned())
    }

    fn message(role: &str, content: &str) -> JsonValue {
        JsonValue::Object(object(&[
            ("role", string(role)),
            ("content", string(content)),
        ]))
    }

    fn minimal(extra: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        let mut body = object(&[
            ("model", string("gpt-x")),
            ("messages", JsonValue::Array(vec![message("user", "hi")])),
        ]);
        for (key, value) in extra {
            body.insert(key.to_string(), value.clone());
        }
        body
    }

    #[test]
    fn a_minimal_request_decodes() {
        let request = decode_chat_request(minimal(&[])).unwrap();
        assert_eq!(request.model, "gpt-x");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, ChatRole::User);
        assert_eq!(request.messages[0].content, "hi");
        assert!(!request.stream);
    }

    #[test]
    fn a_missing_model_is_rejected() {
        let mut body = minimal(&[]);
        body.remove("model");
        assert!(decode_chat_request(body.clone()).is_err());
        body.insert("model".to_owned(), string(""));
        assert!(decode_chat_request(body.clone()).is_err());
    }

    #[test]
    fn empty_or_non_object_messages_are_rejected() {
        let mut body = minimal(&[]);
        body.insert("messages".to_owned(), JsonValue::Array(Vec::new()));
        assert!(decode_chat_request(body.clone()).is_err());
        body.insert(
            "messages".to_owned(),
            JsonValue::Array(vec![string("not an object")]),
        );
        assert!(decode_chat_request(body.clone()).is_err());
    }

    #[test]
    fn an_unknown_parameter_is_rejected() {
        let body = minimal(&[("frobnicate", JsonValue::Bool(true))]);
        let error = decode_chat_request(body.clone()).unwrap_err();
        assert_eq!(error.wire_code().as_deref(), Some("unsupported_parameter"));
    }

    #[test]
    fn an_empty_logit_bias_is_tolerated() {
        let body = minimal(&[("logit_bias", JsonValue::Object(BTreeMap::new()))]);
        assert!(decode_chat_request(body.clone()).is_ok());
        // ...but a non-empty one is an unhonored key.
        let populated = minimal(&[(
            "logit_bias",
            JsonValue::Object(object(&[("50256", JsonValue::Int(-100))])),
        )]);
        assert!(decode_chat_request(populated.clone()).is_err());
    }

    #[test]
    fn developer_role_maps_to_system() {
        let body = minimal(&[(
            "messages",
            JsonValue::Array(vec![message("developer", "sys")]),
        )]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(request.messages[0].role, ChatRole::System);
    }

    #[test]
    fn a_tool_message_requires_a_tool_call_id() {
        let no_id = JsonValue::Object(object(&[
            ("role", string("tool")),
            ("content", string("42")),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![no_id]))]);
        assert!(decode_chat_request(body.clone()).is_err());

        let with_id = JsonValue::Object(object(&[
            ("role", string("tool")),
            ("content", string("42")),
            ("tool_call_id", string("call_1")),
            ("name", string("adder")),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![with_id]))]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(request.messages[0].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(request.messages[0].tool_name.as_deref(), Some("adder"));
    }

    #[test]
    fn sampling_params_are_collected_and_max_tokens_aliases() {
        let body = minimal(&[
            ("temperature", JsonValue::Double(0.5)),
            ("top_p", JsonValue::Int(1)),
            ("max_completion_tokens", JsonValue::Int(256)),
            ("seed", JsonValue::Int(7)),
        ]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(
            request.sampling.get("temperature"),
            Some(&JsonValue::Double(0.5))
        );
        assert_eq!(request.sampling.get("top_p"), Some(&JsonValue::Double(1.0)));
        assert_eq!(
            request.sampling.get("max_tokens"),
            Some(&JsonValue::Int(256))
        );
        assert_eq!(request.sampling.get("seed"), Some(&JsonValue::Int(7)));
    }

    #[test]
    fn explicit_max_tokens_wins_over_the_alias() {
        let body = minimal(&[
            ("max_tokens", JsonValue::Int(100)),
            ("max_completion_tokens", JsonValue::Int(999)),
        ]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(
            request.sampling.get("max_tokens"),
            Some(&JsonValue::Int(100))
        );
    }

    #[test]
    fn n_greater_than_one_is_rejected_but_one_is_fine() {
        let body = minimal(&[("n", JsonValue::Int(2))]);
        assert_eq!(
            decode_chat_request(body.clone())
                .unwrap_err()
                .wire_code()
                .as_deref(),
            Some("unsupported_parameter")
        );
        let body = minimal(&[("n", JsonValue::Int(1))]);
        assert!(decode_chat_request(body.clone()).is_ok());
    }

    #[test]
    fn a_non_numeric_temperature_is_rejected() {
        let body = minimal(&[("temperature", string("warm"))]);
        let error = decode_chat_request(body.clone()).unwrap_err();
        assert_eq!(error.wire_code().as_deref(), Some("invalid_type"));
    }

    #[test]
    fn a_fractional_max_tokens_is_rejected() {
        let body = minimal(&[("max_tokens", JsonValue::Double(2.5))]);
        assert_eq!(
            decode_chat_request(body.clone())
                .unwrap_err()
                .wire_code()
                .as_deref(),
            Some("invalid_type")
        );
        // An integer-valued float is accepted.
        let body = minimal(&[("max_tokens", JsonValue::Double(8.0))]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(request.sampling.get("max_tokens"), Some(&JsonValue::Int(8)));
    }

    #[test]
    fn text_response_format_is_dropped_others_pass_or_reject() {
        let text = JsonValue::Object(object(&[("type", string("text"))]));
        let request = decode_chat_request(minimal(&[("response_format", text)])).unwrap();
        assert!(!request.sampling.contains_key("response_format"));

        let json = JsonValue::Object(object(&[("type", string("json_object"))]));
        let request = decode_chat_request(minimal(&[("response_format", json.clone())])).unwrap();
        assert_eq!(request.sampling.get("response_format"), Some(&json));

        let weird = JsonValue::Object(object(&[("type", string("yaml"))]));
        assert_eq!(
            decode_chat_request(minimal(&[("response_format", weird)]))
                .unwrap_err()
                .wire_code()
                .as_deref(),
            Some("unsupported_parameter")
        );
    }

    #[test]
    fn stream_options_include_usage_is_read() {
        let options = JsonValue::Object(object(&[("include_usage", JsonValue::Bool(true))]));
        let body = minimal(&[
            ("stream", JsonValue::Bool(true)),
            ("stream_options", options),
        ]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert!(request.stream);
        assert!(request.include_usage);
    }

    #[test]
    fn image_content_parts_become_attachments() {
        // "hi" base64 = "aGk=".
        let part_text = JsonValue::Object(object(&[
            ("type", string("text")),
            ("text", string("look:")),
        ]));
        let image_url = JsonValue::Object(object(&[("url", string("data:image/png;base64,aGk="))]));
        let part_image = JsonValue::Object(object(&[
            ("type", string("image_url")),
            ("image_url", image_url),
        ]));
        let msg = JsonValue::Object(object(&[
            ("role", string("user")),
            ("content", JsonValue::Array(vec![part_text, part_image])),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![msg]))]);
        let request = decode_chat_request(body.clone()).unwrap();
        assert_eq!(request.messages[0].content, "look:");
        assert_eq!(request.messages[0].attachments.len(), 1);
        assert_eq!(request.messages[0].attachments[0].data, b"hi");
        assert_eq!(request.messages[0].attachments[0].mime_type, "image/png");
    }

    #[test]
    fn a_non_data_image_url_is_rejected() {
        let error = decode_image_data_uri("https://example.com/cat.png").unwrap_err();
        assert!(error.message.contains("data: URIs"));
    }

    #[test]
    fn an_image_uri_without_base64_is_rejected() {
        assert!(decode_image_data_uri("data:image/png,notbase64").is_err());
    }

    #[test]
    fn a_data_uri_with_no_mediatype_defaults_to_png() {
        let attachment = decode_image_data_uri("data:;base64,aGk=").unwrap();
        assert_eq!(attachment.mime_type, "image/png");
    }

    #[test]
    fn assistant_tool_calls_decode_with_string_arguments() {
        let call = JsonValue::Object(object(&[
            ("id", string("call_9")),
            (
                "function",
                JsonValue::Object(object(&[
                    ("name", string("adder")),
                    ("arguments", string(r#"{"a":1}"#)),
                ])),
            ),
        ]));
        let msg = JsonValue::Object(object(&[
            ("role", string("assistant")),
            ("content", JsonValue::Null),
            ("tool_calls", JsonValue::Array(vec![call])),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![msg]))]);
        let request = decode_chat_request(body.clone()).unwrap();
        let calls = &request.messages[0].tool_calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_9");
        assert_eq!(calls[0].name, "adder");
        assert_eq!(
            calls[0].arguments,
            JsonValue::Object(object(&[("a", JsonValue::Int(1))]))
        );
    }

    #[test]
    fn malformed_string_tool_arguments_are_rejected() {
        let call = JsonValue::Object(object(&[(
            "function",
            JsonValue::Object(object(&[
                ("name", string("f")),
                ("arguments", string("not json")),
            ])),
        )]));
        let msg = JsonValue::Object(object(&[
            ("role", string("assistant")),
            ("content", string("")),
            ("tool_calls", JsonValue::Array(vec![call])),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![msg]))]);
        assert!(decode_chat_request(body.clone()).is_err());
    }

    #[test]
    fn a_bad_tool_choice_is_rejected_but_a_string_or_object_passes() {
        let body = minimal(&[("tool_choice", JsonValue::Bool(true))]);
        assert!(decode_chat_request(body.clone()).is_err());
        let body = minimal(&[("tool_choice", string("auto"))]);
        assert_eq!(
            decode_chat_request(body.clone()).unwrap().tool_choice,
            Some(string("auto"))
        );
        let choice = JsonValue::Object(object(&[("type", string("function"))]));
        let body = minimal(&[("tool_choice", choice.clone())]);
        assert_eq!(decode_chat_request(body).unwrap().tool_choice, Some(choice));
    }

    #[test]
    fn a_present_but_empty_tool_call_id_is_kept_verbatim() {
        // Swift honors an empty id rather than minting a fresh one, so a later
        // tool result can still match it.
        let call = JsonValue::Object(object(&[
            ("id", string("")),
            (
                "function",
                JsonValue::Object(object(&[
                    ("name", string("f")),
                    ("arguments", JsonValue::Object(BTreeMap::new())),
                ])),
            ),
        ]));
        let msg = JsonValue::Object(object(&[
            ("role", string("assistant")),
            ("content", string("")),
            ("tool_calls", JsonValue::Array(vec![call])),
        ]));
        let body = minimal(&[("messages", JsonValue::Array(vec![msg]))]);
        let request = decode_chat_request(body).unwrap();
        assert_eq!(request.messages[0].tool_calls[0].id, "");
    }

    #[test]
    fn an_out_of_range_or_lossy_integer_param_is_rejected() {
        // 1e19 overflows i64, so it parses as a Double; as a seed it must be
        // rejected, not saturated.
        let body = minimal(&[("seed", JsonValue::Double(1e19))]);
        assert_eq!(
            decode_chat_request(body)
                .unwrap_err()
                .wire_code()
                .as_deref(),
            Some("invalid_type")
        );
        // An integer too large to represent exactly as a float is rejected as a
        // number param, matching Swift's exact bridging.
        let body = minimal(&[("temperature", JsonValue::Int(9_007_199_254_740_993))]);
        assert_eq!(
            decode_chat_request(body)
                .unwrap_err()
                .wire_code()
                .as_deref(),
            Some("invalid_type")
        );
    }

    #[test]
    fn the_system_role_maps_to_system() {
        let body = minimal(&[(
            "messages",
            JsonValue::Array(vec![message("system", "be nice")]),
        )]);
        assert_eq!(
            decode_chat_request(body).unwrap().messages[0].role,
            ChatRole::System
        );
    }

    #[test]
    fn malformed_content_parts_are_rejected() {
        let unknown = JsonValue::Object(object(&[("type", string("audio_url"))]));
        let msg = JsonValue::Object(object(&[
            ("role", string("user")),
            ("content", JsonValue::Array(vec![unknown])),
        ]));
        assert!(
            decode_chat_request(minimal(&[("messages", JsonValue::Array(vec![msg]))])).is_err()
        );

        let text_missing = JsonValue::Object(object(&[("type", string("text"))]));
        let msg = JsonValue::Object(object(&[
            ("role", string("user")),
            ("content", JsonValue::Array(vec![text_missing])),
        ]));
        assert!(
            decode_chat_request(minimal(&[("messages", JsonValue::Array(vec![msg]))])).is_err()
        );

        let image_missing = JsonValue::Object(object(&[("type", string("image_url"))]));
        let msg = JsonValue::Object(object(&[
            ("role", string("user")),
            ("content", JsonValue::Array(vec![image_missing])),
        ]));
        assert!(
            decode_chat_request(minimal(&[("messages", JsonValue::Array(vec![msg]))])).is_err()
        );
    }
}
