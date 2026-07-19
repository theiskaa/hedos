//! Decoding an Ollama `/api/chat` request body into the kernel's chat model: the
//! messages (with inline base64 images), the `options` block mapped onto the
//! kernel's parameter names, the `think`/`format` directives, and tool calls.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use kernel::capabilities::{
    AttachmentKind, ChatAttachment, ChatMessage, ChatRole, GenerationStats, ToolCall, ToolSpec,
    decode_tool_specs,
};
use kernel::records::{JsonValue, ModelRecord};
use regex::Regex;
use serde_json::{Value, json};

use crate::error::{GatewayError, GatewayErrorKind};
use crate::wire::{param_decoding, timestamp};
use base64::prelude::{BASE64_STANDARD, Engine as _};

/// The number of bytes in a mebibyte, for the `size` field.
const BYTES_PER_MIB: i64 = 1_048_576;

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
                .and_then(|image| BASE64_STANDARD.decode(image).ok())
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
        // BTreeMap iterates in sorted key order (deterministic).
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
/// integer (integer form is preferred), else a float. Anything else is a bad
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
        // A present string id is honored verbatim (only presence is checked).
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

/// One `tool_calls` entry for a `/api/chat` message: `{function: {name,
/// arguments}}` with arguments as a nested object (the Ollama wire shape).
fn tool_call_entry(call: &ToolCall) -> Value {
    json!({
        "function": {
            "name": call.name,
            "arguments": serde_json::to_value(&call.arguments).unwrap_or(Value::Null),
        }
    })
}

/// Serialize a value as a newline-terminated NDJSON line (the Ollama streaming
/// frame format).
pub fn line(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// A streaming `/api/chat` delta frame (`done: false`).
pub fn delta(
    model: &str,
    created_at: &str,
    content: Option<&str>,
    thinking: Option<&str>,
    tool_call: Option<&ToolCall>,
) -> Value {
    let mut message = serde_json::Map::new();
    message.insert("role".to_owned(), json!("assistant"));
    message.insert("content".to_owned(), json!(content.unwrap_or("")));
    if let Some(thinking) = thinking {
        message.insert("thinking".to_owned(), json!(thinking));
    }
    if let Some(call) = tool_call {
        message.insert("tool_calls".to_owned(), json!([tool_call_entry(call)]));
    }
    json!({
        "model": model,
        "created_at": created_at,
        "message": Value::Object(message),
        "done": false,
    })
}

/// The terminal `/api/chat` frame (`done: true`), carrying the finish reason and
/// any generation stats.
pub fn final_frame(
    model: &str,
    created_at: &str,
    content: &str,
    stats: Option<&GenerationStats>,
    tool_calls: &[ToolCall],
) -> Value {
    let mut message = serde_json::Map::new();
    message.insert("role".to_owned(), json!("assistant"));
    message.insert("content".to_owned(), json!(content));
    if !tool_calls.is_empty() {
        let calls: Vec<Value> = tool_calls.iter().map(tool_call_entry).collect();
        message.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    let finish = stats.and_then(|stats| stats.finish_reason.as_deref());
    // Ollama never reports tool_calls as a done_reason; it maps to "stop".
    let done_reason = if !tool_calls.is_empty() || finish == Some("tool_calls") {
        "stop"
    } else {
        finish.unwrap_or("stop")
    };
    let mut frame = serde_json::Map::new();
    frame.insert("model".to_owned(), json!(model));
    frame.insert("created_at".to_owned(), json!(created_at));
    frame.insert("message".to_owned(), Value::Object(message));
    frame.insert("done".to_owned(), json!(true));
    frame.insert("done_reason".to_owned(), json!(done_reason));
    insert_stats(&mut frame, stats);
    Value::Object(frame)
}

/// A streaming `/api/generate` delta frame (`done: false`).
pub fn generate_delta(model: &str, created_at: &str, response: &str) -> Value {
    json!({
        "model": model,
        "created_at": created_at,
        "response": response,
        "done": false,
    })
}

/// The terminal `/api/generate` frame (`done: true`).
pub fn generate_final(model: &str, created_at: &str, stats: Option<&GenerationStats>) -> Value {
    let done_reason = stats
        .and_then(|stats| stats.finish_reason.as_deref())
        .unwrap_or("stop");
    let mut frame = serde_json::Map::new();
    frame.insert("model".to_owned(), json!(model));
    frame.insert("created_at".to_owned(), json!(created_at));
    frame.insert("response".to_owned(), json!(""));
    frame.insert("done".to_owned(), json!(true));
    frame.insert("done_reason".to_owned(), json!(done_reason));
    insert_stats(&mut frame, stats);
    Value::Object(frame)
}

/// Add the optional `total_duration`/`prompt_eval_count`/`eval_count` fields for
/// whichever stats are present. Duration is nanoseconds on the Ollama wire.
fn insert_stats(frame: &mut serde_json::Map<String, Value>, stats: Option<&GenerationStats>) {
    let Some(stats) = stats else {
        return;
    };
    if let Some(duration_ms) = stats.duration_ms {
        frame.insert("total_duration".to_owned(), json!(duration_ms * 1_000_000));
    }
    if let Some(prompt_tokens) = stats.prompt_tokens {
        frame.insert("prompt_eval_count".to_owned(), json!(prompt_tokens));
    }
    if let Some(completion_tokens) = stats.completion_tokens {
        frame.insert("eval_count".to_owned(), json!(completion_tokens));
    }
}

/// Matches a parameter-size token like `7b` or `1.5B` or `70m`.
static PARAMETER_SIZE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"[0-9]+(\.[0-9]+)?[bBmM]").ok());

/// The quantization-level patterns, tried in order (e.g. `Q4_K_M`, then `Q8`,
/// then `F16`/`BF16`).
static QUANT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"[Qq][0-9]_[0-9A-Za-z_]+",
        r"[Qq][0-9]+",
        r"[Bb]?[Ff](16|32)",
    ]
    .into_iter()
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

/// The `/api/tags` list body for a shelf of records.
pub fn tags(records: &[ModelRecord]) -> Value {
    let models: Vec<Value> = records
        .iter()
        .map(|record| {
            let name = record.alias.as_deref().unwrap_or(&record.name);
            json!({
                "name": name,
                "model": name,
                "modified_at": timestamp::iso8601(record.registered_at),
                "size": record.footprint_mb.unwrap_or(0) * BYTES_PER_MIB,
                "digest": "",
                "details": details(record),
            })
        })
        .collect();
    json!({ "models": models })
}

/// The `details` sub-object for one record: its weight format and the parameter
/// size and quantization level guessed from its name and weight path.
pub fn details(record: &ModelRecord) -> Value {
    let format = record
        .primary_weight_path
        .as_deref()
        .and_then(|path| Path::new(path).extension())
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let tokens = format!(
        "{} {}",
        record.name,
        record.primary_weight_path.as_deref().unwrap_or("")
    );
    json!({
        "parent_model": "",
        "format": format,
        "family": "",
        "families": [],
        "parameter_size": parameter_size(&tokens),
        "quantization_level": quantization_level(&tokens),
    })
}

/// The first parameter-size token in `text`, upper-cased, or empty if none.
fn parameter_size(text: &str) -> String {
    PARAMETER_SIZE
        .as_ref()
        .and_then(|pattern| pattern.find(text))
        .map(|matched| matched.as_str().to_uppercase())
        .unwrap_or_default()
}

/// The first quantization-level token in `text` (trying each pattern in order),
/// upper-cased, or empty if none.
fn quantization_level(text: &str) -> String {
    for pattern in QUANT_PATTERNS.iter() {
        if let Some(matched) = pattern.find(text) {
            return matched.as_str().to_uppercase();
        }
    }
    String::new()
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
        // 1e19 overflows i64, so it falls back to a float and must not be
        // saturated to i64::MAX.
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

    fn tool_call(name: &str, args: &[(&str, JsonValue)]) -> ToolCall {
        ToolCall::with_id("call_1", name, JsonValue::Object(map(args)))
    }

    #[test]
    fn a_line_is_newline_terminated_ndjson() {
        assert_eq!(line(&json!({"a": 1})), b"{\"a\":1}\n");
    }

    #[test]
    fn a_delta_frame_carries_content_and_is_not_done() {
        let value = delta("llama3", "2020-01-01T00:00:00Z", Some("hi"), None, None);
        assert_eq!(value["model"], "llama3");
        assert_eq!(value["created_at"], "2020-01-01T00:00:00Z");
        assert_eq!(value["message"]["role"], "assistant");
        assert_eq!(value["message"]["content"], "hi");
        assert_eq!(value["done"], false);
    }

    #[test]
    fn a_delta_defaults_content_and_can_carry_thinking_and_a_tool_call() {
        let call = tool_call("adder", &[("a", JsonValue::Int(1))]);
        let value = delta("m", "t", None, Some("hmm"), Some(&call));
        assert_eq!(value["message"]["content"], "");
        assert_eq!(value["message"]["thinking"], "hmm");
        let entry = &value["message"]["tool_calls"][0];
        assert_eq!(entry["function"]["name"], "adder");
        // Arguments are a nested object, not a JSON string.
        assert_eq!(entry["function"]["arguments"]["a"], 1);
    }

    #[test]
    fn a_final_frame_maps_tool_calls_to_a_stop_reason_and_carries_stats() {
        let call = tool_call("adder", &[]);
        let stats = GenerationStats {
            prompt_tokens: Some(3),
            completion_tokens: Some(4),
            duration_ms: Some(10),
            ..Default::default()
        };
        let value = final_frame("m", "t", "", Some(&stats), std::slice::from_ref(&call));
        assert_eq!(value["done"], true);
        // Tool calls present → done_reason "stop", never "tool_calls".
        assert_eq!(value["done_reason"], "stop");
        assert_eq!(
            value["message"]["tool_calls"][0]["function"]["name"],
            "adder"
        );
        assert_eq!(value["total_duration"], 10_000_000i64);
        assert_eq!(value["prompt_eval_count"], 3);
        assert_eq!(value["eval_count"], 4);
    }

    #[test]
    fn a_plain_final_frame_uses_the_stats_finish_reason() {
        let stats = GenerationStats {
            finish_reason: Some("length".to_owned()),
            ..Default::default()
        };
        let value = final_frame("m", "t", "done", Some(&stats), &[]);
        assert_eq!(value["done_reason"], "length");
        assert_eq!(value["message"]["content"], "done");
        // No duration/token stats → those keys are absent.
        assert!(value.get("total_duration").is_none());
    }

    #[test]
    fn a_tool_calls_finish_reason_from_stats_also_maps_to_stop() {
        let stats = GenerationStats {
            finish_reason: Some("tool_calls".to_owned()),
            ..Default::default()
        };
        let value = final_frame("m", "t", "", Some(&stats), &[]);
        assert_eq!(value["done_reason"], "stop");
    }

    #[test]
    fn generate_frames_carry_the_response_field() {
        let delta = generate_delta("m", "t", "chunk");
        assert_eq!(delta["response"], "chunk");
        assert_eq!(delta["done"], false);

        let stats = GenerationStats {
            completion_tokens: Some(9),
            ..Default::default()
        };
        let final_frame = generate_final("m", "t", Some(&stats));
        assert_eq!(final_frame["response"], "");
        assert_eq!(final_frame["done"], true);
        assert_eq!(final_frame["done_reason"], "stop");
        assert_eq!(final_frame["eval_count"], 9);
    }

    fn record(name: &str, weight_path: Option<&str>) -> ModelRecord {
        use kernel::records::{Modality, ModelSource, SourceKind};
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::file(), weight_path.unwrap_or("")),
        );
        record.primary_weight_path = weight_path.map(str::to_owned);
        record
    }

    #[test]
    fn tags_lists_a_record_with_a_size_and_iso_modified_at() {
        let mut rec = record("llama3", Some("/models/llama3-8b-q4_k_m.gguf"));
        rec.footprint_mb = Some(4096);
        rec.registered_at = 1_600_000_000_000;
        let value = tags(std::slice::from_ref(&rec));
        let entry = &value["models"][0];
        assert_eq!(entry["name"], "llama3");
        assert_eq!(entry["model"], "llama3");
        assert_eq!(entry["modified_at"], "2020-09-13T12:26:40Z");
        assert_eq!(entry["size"], 4096i64 * 1_048_576);
        assert_eq!(entry["digest"], "");
    }

    #[test]
    fn tags_prefers_the_alias_for_the_name() {
        let mut rec = record("raw", None);
        rec.alias = Some("Friendly".to_owned());
        let value = tags(std::slice::from_ref(&rec));
        assert_eq!(value["models"][0]["name"], "Friendly");
    }

    #[test]
    fn details_reads_the_format_size_and_quantization() {
        let rec = record("llama3-8b", Some("/models/llama3-8B-Q4_K_M.gguf"));
        let value = details(&rec);
        assert_eq!(value["format"], "gguf");
        assert_eq!(value["parameter_size"], "8B");
        assert_eq!(value["quantization_level"], "Q4_K_M");
        assert_eq!(value["families"], json!([]));
    }

    #[test]
    fn details_falls_back_to_empty_when_nothing_matches() {
        let rec = record("plain-model", None);
        let value = details(&rec);
        assert_eq!(value["format"], "");
        assert_eq!(value["parameter_size"], "");
        assert_eq!(value["quantization_level"], "");
    }

    #[test]
    fn parameter_size_and_quantization_extraction() {
        assert_eq!(parameter_size("mistral-7b-instruct"), "7B");
        assert_eq!(parameter_size("gemma-1.5b"), "1.5B");
        assert_eq!(parameter_size("phi-3-mini"), "");
        assert_eq!(quantization_level("model-q8_0.gguf"), "Q8_0");
        assert_eq!(quantization_level("model-q4.bin"), "Q4");
        assert_eq!(quantization_level("model-f16.safetensors"), "F16");
        assert_eq!(quantization_level("model-bf16"), "BF16");
        assert_eq!(quantization_level("plain"), "");
    }
}
