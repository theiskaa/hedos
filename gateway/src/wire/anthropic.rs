//! Decoding an Anthropic `/v1/messages` request into the kernel's chat model and
//! encoding the reply, both as a single message and as Anthropic's SSE grammar.
//!
//! Unlike the OpenAI surface, unknown top-level keys are ignored rather than
//! rejected. Claude Code sends fields the gateway does not implement —
//! `thinking`, `context_management`, `output_config`, `cache_control` — to any
//! model name it does not recognize, which is every alias hedos serves. A strict
//! reader would 400 on all of them.

use std::collections::BTreeMap;

use kernel::capabilities::{ChatMessage, ChatRole, GenerationStats, ToolCall, ToolSpec};
use kernel::records::JsonValue;
use serde_json::{Value, json};

use crate::error::{GatewayError, GatewayErrorKind};
use crate::wire::param_decoding;

/// A decoded messages request.
#[derive(Debug, Clone, PartialEq)]
pub struct MessagesRequest {
    /// The requested model id or alias.
    pub model: String,
    /// The conversation so far, with the system prompt folded in first.
    pub messages: Vec<ChatMessage>,
    /// Whether the response should stream as SSE.
    pub stream: bool,
    /// The honored sampling parameters, keyed by wire name.
    pub sampling: BTreeMap<String, JsonValue>,
    /// The tools the model may call. Already cleared when the request said
    /// `tool_choice: {"type": "none"}`.
    pub tools: Vec<ToolSpec>,
    /// The `tool_choice` directive, translated to the kernel's (OpenAI-shaped)
    /// form. `None` for absent, `auto`, or `none` (which clears `tools` instead).
    pub tool_choice: Option<JsonValue>,
}

fn bad_request(message: impl Into<String>) -> GatewayError {
    GatewayError::new(GatewayErrorKind::BadRequest, message)
}

/// Decode a messages request from its JSON object body.
pub fn decode_messages_request(
    body: BTreeMap<String, JsonValue>,
) -> Result<MessagesRequest, GatewayError> {
    let model = body
        .get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))?
        .to_owned();

    let raw = body
        .get("messages")
        .and_then(JsonValue::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(|| bad_request("messages is required"))?;

    let mut messages = Vec::with_capacity(raw.len() + 1);
    if let Some(system) = system_text(body.get("system")) {
        messages.push(ChatMessage::new(ChatRole::System, system));
    }
    for entry in raw {
        decode_message(entry, &mut messages)?;
    }

    let mut tools = decode_tools(body.get("tools"));
    let tool_choice = decode_tool_choice(body.get("tool_choice"), &mut tools);

    Ok(MessagesRequest {
        model,
        messages,
        stream: body
            .get("stream")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false),
        sampling: decode_sampling(&body)?,
        tools,
        tool_choice,
    })
}

/// Translate `tool_choice` to the kernel's OpenAI-shaped form: `none` clears
/// `tools` (the model must not see them at all), `any` becomes `required`, and
/// a forced `tool` becomes the function form. `auto` — and, per this module's
/// lenient posture, anything unrecognized — is the default and translates to
/// nothing.
fn decode_tool_choice(value: Option<&JsonValue>, tools: &mut Vec<ToolSpec>) -> Option<JsonValue> {
    let fields = value?.as_object()?;
    match fields.get("type").and_then(JsonValue::as_str)? {
        "none" => {
            tools.clear();
            None
        }
        "any" => Some(JsonValue::String("required".to_owned())),
        "tool" => {
            let name = fields.get("name").and_then(JsonValue::as_str)?;
            let function =
                BTreeMap::from([("name".to_owned(), JsonValue::String(name.to_owned()))]);
            Some(JsonValue::Object(BTreeMap::from([
                ("type".to_owned(), JsonValue::String("function".to_owned())),
                ("function".to_owned(), JsonValue::Object(function)),
            ])))
        }
        _ => None,
    }
}

/// The system prompt as flat text. Anthropic allows either a bare string or an
/// array of text blocks; Claude Code sends the array form, with its attribution
/// block first.
fn system_text(value: Option<&JsonValue>) -> Option<String> {
    match value? {
        JsonValue::String(text) if !text.is_empty() => Some(text.clone()),
        JsonValue::Array(blocks) => {
            let joined = blocks
                .iter()
                .filter_map(|block| block.as_object())
                .filter_map(|fields| fields.get("text").and_then(JsonValue::as_str))
                .collect::<Vec<_>>()
                .join("\n\n");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

/// Decode one wire message, pushing one or more kernel messages.
///
/// A single Anthropic turn can carry both assistant text and tool calls, and a
/// user turn can carry several tool results. Tool results become their own
/// `Tool` messages, which is the shape the kernel's chat model expects.
fn decode_message(value: &JsonValue, into: &mut Vec<ChatMessage>) -> Result<(), GatewayError> {
    let fields = value
        .as_object()
        .ok_or_else(|| bad_request("each message must be an object"))?;
    let role = fields
        .get("role")
        .and_then(JsonValue::as_str)
        .and_then(ChatRole::from_wire)
        .ok_or_else(|| bad_request("each message needs a role of \"user\" or \"assistant\""))?;

    let content = fields.get("content");
    if let Some(JsonValue::String(text)) = content {
        into.push(ChatMessage::new(role, text.clone()));
        return Ok(());
    }

    let Some(JsonValue::Array(blocks)) = content else {
        return Err(bad_request("message content must be a string or an array"));
    };

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut results = Vec::new();
    let mut dropped: Vec<&str> = Vec::new();
    for block in blocks {
        let Some(fields) = block.as_object() else {
            continue;
        };
        match fields.get("type").and_then(JsonValue::as_str) {
            Some("text") => {
                if let Some(chunk) = fields.get("text").and_then(JsonValue::as_str) {
                    text.push_str(chunk);
                }
            }
            Some("tool_use") => {
                if let Some(call) = decode_tool_use(fields) {
                    tool_calls.push(call);
                }
            }
            Some("tool_result") => results.push(decode_tool_result(fields)),
            // Thinking blocks are echoed back from a previous turn and carry a
            // signature this gateway never issued; there is nothing to replay.
            Some("thinking") | Some("redacted_thinking") | None => {}
            Some(other) => dropped.push(other),
        }
    }

    if !text.is_empty() || !tool_calls.is_empty() {
        let mut message = ChatMessage::new(role, text);
        message.tool_calls = tool_calls;
        into.push(message);
    } else if results.is_empty() && !dropped.is_empty() {
        // The turn held only content this surface cannot decode — most commonly
        // an image paste. Dropping the whole message would splice it out of the
        // conversation and leave the model continuing from its own previous
        // turn, so a placeholder keeps the turn present and says what was there.
        dropped.dedup();
        into.push(ChatMessage::new(
            role,
            format!("[unsupported content: {}]", dropped.join(", ")),
        ));
    }
    for (id, body) in results {
        let mut message = ChatMessage::new(ChatRole::Tool, body);
        message.tool_call_id = Some(id);
        into.push(message);
    }
    Ok(())
}

/// A `tool_use` block as a kernel tool call.
fn decode_tool_use(fields: &BTreeMap<String, JsonValue>) -> Option<ToolCall> {
    let id = fields.get("id").and_then(JsonValue::as_str)?;
    let name = fields.get("name").and_then(JsonValue::as_str)?;
    Some(ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: fields
            .get("input")
            .cloned()
            .unwrap_or_else(|| JsonValue::Object(BTreeMap::new())),
    })
}

/// A `tool_result` block as its call id and flattened text body.
fn decode_tool_result(fields: &BTreeMap<String, JsonValue>) -> (String, String) {
    let id = fields
        .get("tool_use_id")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_owned();
    let body = match fields.get("content") {
        Some(JsonValue::String(text)) => text.clone(),
        Some(JsonValue::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.as_object())
            .filter_map(|fields| fields.get("text").and_then(JsonValue::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    (id, body)
}

/// The tool specs, dropping any entry without a name.
///
/// Anthropic names the schema `input_schema` where OpenAI nests `parameters`
/// under a `function` object, so this cannot reuse the OpenAI decoder.
fn decode_tools(value: Option<&JsonValue>) -> Vec<ToolSpec> {
    let Some(JsonValue::Array(entries)) = value else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| entry.as_object())
        .filter_map(|fields| {
            let name = fields.get("name").and_then(JsonValue::as_str)?;
            Some(ToolSpec::new(
                name,
                fields
                    .get("description")
                    .and_then(JsonValue::as_str)
                    .unwrap_or(""),
                fields
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| JsonValue::Object(BTreeMap::new())),
            ))
        })
        .collect()
}

/// The sampling parameters this surface honors.
///
/// The shared coercions are used with their errors discarded: where the OpenAI
/// surface 400s a wrong-typed value, this surface ignores it, per the module's
/// lenient posture.
fn decode_sampling(
    body: &BTreeMap<String, JsonValue>,
) -> Result<BTreeMap<String, JsonValue>, GatewayError> {
    let mut sampling = BTreeMap::new();
    for key in ["max_tokens", "top_k"] {
        if let Ok(Some(value)) = param_decoding::int_param(body, key) {
            sampling.insert(key.to_owned(), JsonValue::Int(value));
        }
    }
    for key in ["temperature", "top_p"] {
        if let Ok(Some(value)) = param_decoding::number_param(body, key) {
            sampling.insert(key.to_owned(), JsonValue::Double(value));
        }
    }
    if let Some(stop) = param_decoding::stop(body.get("stop_sequences"), None)? {
        sampling.insert("stop".to_owned(), stop);
    }
    Ok(sampling)
}

/// The kernel chat payload for a decoded request.
pub fn messages_payload(request: &MessagesRequest) -> JsonValue {
    let mut payload = BTreeMap::new();
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
        if let Some(tool_choice) = &request.tool_choice {
            payload.insert("tool_choice".to_owned(), tool_choice.clone());
        }
    }
    for (key, value) in &request.sampling {
        payload.insert(key.clone(), value.clone());
    }
    JsonValue::Object(payload)
}

/// Anthropic's stop reason for a finished generation.
///
/// A tool call always wins: Claude Code drives its agent loop off `tool_use`,
/// and reporting `end_turn` alongside a tool call would end the turn instead.
pub fn stop_reason(tool_calls: usize, stats: Option<&GenerationStats>) -> &'static str {
    if tool_calls > 0 {
        return "tool_use";
    }
    match stats.and_then(|stats| stats.finish_reason.as_deref()) {
        Some("length") => "max_tokens",
        Some("stop_sequence") => "stop_sequence",
        _ => "end_turn",
    }
}

/// The `usage` object. Anthropic names the counts differently from OpenAI and
/// omits a total.
pub fn usage(stats: Option<&GenerationStats>) -> Value {
    json!({
        "input_tokens": stats.and_then(|stats| stats.prompt_tokens).unwrap_or(0),
        "output_tokens": stats.and_then(|stats| stats.completion_tokens).unwrap_or(0),
    })
}

/// The content blocks for a complete reply.
fn content_blocks(text: &str, tool_calls: &[ToolCall]) -> Vec<Value> {
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    for call in tool_calls {
        blocks.push(tool_use_block(call));
    }
    blocks
}

/// One `tool_use` content block.
fn tool_use_block(call: &ToolCall) -> Value {
    json!({
        "type": "tool_use",
        "id": call.id,
        "name": call.name,
        "input": serde_json::to_value(&call.arguments).unwrap_or_else(|_| json!({})),
    })
}

/// A complete, non-streamed message response.
pub fn message(
    id: &str,
    model: &str,
    text: &str,
    tool_calls: &[ToolCall],
    stats: Option<&GenerationStats>,
) -> Value {
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content_blocks(text, tool_calls),
        "stop_reason": stop_reason(tool_calls.len(), stats),
        "stop_sequence": Value::Null,
        "usage": usage(stats),
    })
}

/// An SSE frame. Anthropic names the event in an `event:` line as well as in the
/// payload's `type`, and clients read the header.
pub fn sse_frame(event: &str, value: &Value) -> Vec<u8> {
    format!(
        "event: {event}\ndata: {}\n\n",
        serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
    )
    .into_bytes()
}

/// The opening `message_start` frame.
pub fn message_start(id: &str, model: &str, stats: Option<&GenerationStats>) -> Vec<u8> {
    sse_frame(
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": usage(stats),
            },
        }),
    )
}

/// A `content_block_start` frame opening a text block.
pub fn text_block_start(index: usize) -> Vec<u8> {
    sse_frame(
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": index,
            "content_block": { "type": "text", "text": "" },
        }),
    )
}

/// A `content_block_delta` frame carrying more text.
pub fn text_delta(index: usize, text: &str) -> Vec<u8> {
    sse_frame(
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": index,
            "delta": { "type": "text_delta", "text": text },
        }),
    )
}

/// A `content_block_start` frame opening a tool-use block.
pub fn tool_block_start(index: usize, call: &ToolCall) -> Vec<u8> {
    sse_frame(
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": {},
            },
        }),
    )
}

/// A `content_block_delta` frame carrying a tool call's arguments.
///
/// The kernel hands over a complete call rather than a token stream, so the
/// whole argument object goes out as one `input_json_delta`. Clients accumulate
/// the partials and parse at `content_block_stop`, so one is as valid as many.
pub fn tool_input_delta(index: usize, call: &ToolCall) -> Vec<u8> {
    let partial = serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".to_owned());
    sse_frame(
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": index,
            "delta": { "type": "input_json_delta", "partial_json": partial },
        }),
    )
}

/// A `content_block_stop` frame.
pub fn block_stop(index: usize) -> Vec<u8> {
    sse_frame(
        "content_block_stop",
        &json!({ "type": "content_block_stop", "index": index }),
    )
}

/// The `message_delta` frame carrying the stop reason and output token count.
pub fn message_delta(tool_calls: usize, stats: Option<&GenerationStats>) -> Vec<u8> {
    sse_frame(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": stop_reason(tool_calls, stats),
                "stop_sequence": Value::Null,
            },
            "usage": {
                "output_tokens": stats.and_then(|stats| stats.completion_tokens).unwrap_or(0),
            },
        }),
    )
}

/// The terminal `message_stop` frame.
pub fn message_stop() -> Vec<u8> {
    sse_frame("message_stop", &json!({ "type": "message_stop" }))
}

/// An `error` frame, for a failure after the stream has already begun.
pub fn error_frame(kind: GatewayErrorKind, message: &str) -> Vec<u8> {
    sse_frame(
        "error",
        &json!({
            "type": "error",
            "error": { "type": kind.anthropic_type(), "message": message },
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(value: Value) -> BTreeMap<String, JsonValue> {
        match serde_json::from_value::<JsonValue>(value).expect("convertible") {
            JsonValue::Object(fields) => fields,
            _ => panic!("expected an object"),
        }
    }

    #[test]
    fn a_string_system_prompt_leads_the_conversation() {
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "system": "be terse",
            "messages": [{ "role": "user", "content": "hi" }],
        })))
        .expect("decodes");
        assert_eq!(request.messages[0].role, ChatRole::System);
        assert_eq!(request.messages[0].content, "be terse");
        assert_eq!(request.messages[1].role, ChatRole::User);
    }

    #[test]
    fn a_block_array_system_prompt_is_joined() {
        // Claude Code sends the array form, with its attribution block first.
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "system": [
                { "type": "text", "text": "first" },
                { "type": "text", "text": "second" },
            ],
            "messages": [{ "role": "user", "content": "hi" }],
        })))
        .expect("decodes");
        assert_eq!(request.messages[0].content, "first\n\nsecond");
    }

    #[test]
    fn unknown_top_level_keys_are_ignored() {
        // Claude Code sends all of these to any model name it doesn't know,
        // which is every alias hedos serves. Rejecting them would 400 the
        // entire session.
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{ "role": "user", "content": "hi" }],
            "thinking": { "type": "adaptive" },
            "context_management": { "edits": [] },
            "output_config": { "effort": "high" },
            "metadata": { "user_id": "x" },
        })))
        .expect("decodes despite the extra keys");
        assert_eq!(request.model, "qwen3");
    }

    #[test]
    fn an_assistant_turn_carries_text_and_tool_calls_together() {
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "checking" },
                    { "type": "tool_use", "id": "toolu_1", "name": "read", "input": { "path": "a" } },
                ],
            }],
        })))
        .expect("decodes");
        let message = &request.messages[0];
        assert_eq!(message.content, "checking");
        assert_eq!(message.tool_calls.len(), 1);
        assert_eq!(message.tool_calls[0].name, "read");
    }

    #[test]
    fn tool_results_become_their_own_tool_messages() {
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "file body" },
                ],
            }],
        })))
        .expect("decodes");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, ChatRole::Tool);
        assert_eq!(request.messages[0].content, "file body");
        assert_eq!(request.messages[0].tool_call_id.as_deref(), Some("toolu_1"));
    }

    #[test]
    fn tools_are_read_from_input_schema() {
        // Anthropic puts the schema at `input_schema`; OpenAI nests it under a
        // `function` object, so the OpenAI decoder cannot be reused.
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{
                "name": "read",
                "description": "read a file",
                "input_schema": { "type": "object" },
            }],
        })))
        .expect("decodes");
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "read");
        assert_eq!(request.tools[0].description, "read a file");
    }

    #[test]
    fn tool_choice_none_withholds_the_tools_entirely() {
        // A client that disabled tools must never receive tool_use blocks, so
        // the tools are cleared before the model can see them.
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{ "role": "user", "content": "hi" }],
            "tool_choice": { "type": "none" },
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
        })))
        .expect("decodes");
        assert!(request.tools.is_empty());
        assert!(request.tool_choice.is_none());
    }

    #[test]
    fn a_forced_tool_choice_translates_to_the_kernel_form() {
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{ "role": "user", "content": "hi" }],
            "tool_choice": { "type": "tool", "name": "read" },
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
        })))
        .expect("decodes");
        let payload = messages_payload(&request);
        let choice = payload
            .as_object()
            .and_then(|fields| fields.get("tool_choice"))
            .and_then(JsonValue::as_object)
            .expect("a tool_choice object");
        assert_eq!(
            choice.get("type"),
            Some(&JsonValue::String("function".to_owned()))
        );

        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{ "role": "user", "content": "hi" }],
            "tool_choice": { "type": "any" },
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
        })))
        .expect("decodes");
        assert_eq!(
            request.tool_choice,
            Some(JsonValue::String("required".to_owned()))
        );
    }

    #[test]
    fn an_image_only_user_turn_stays_in_the_conversation() {
        // Claude Code sends an image block for a pasted screenshot. This surface
        // cannot decode it, but silently dropping the whole turn would leave the
        // model continuing from its own previous message.
        let request = decode_messages_request(body(json!({
            "model": "qwen3",
            "messages": [{
                "role": "user",
                "content": [{ "type": "image", "source": { "type": "base64", "data": "" } }],
            }],
        })))
        .expect("decodes");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, ChatRole::User);
        assert!(request.messages[0].content.contains("image"));
    }

    #[test]
    fn a_missing_model_or_messages_is_rejected() {
        assert!(decode_messages_request(body(json!({ "messages": [] }))).is_err());
        assert!(decode_messages_request(body(json!({ "model": "qwen3" }))).is_err());
    }

    #[test]
    fn a_tool_call_reports_tool_use_over_the_finish_reason() {
        let stats = GenerationStats {
            finish_reason: Some("stop".to_owned()),
            ..Default::default()
        };
        // Claude Code drives its agent loop off tool_use; end_turn would stop it.
        assert_eq!(stop_reason(1, Some(&stats)), "tool_use");
        assert_eq!(stop_reason(0, Some(&stats)), "end_turn");
    }

    #[test]
    fn a_length_finish_maps_to_max_tokens() {
        let stats = GenerationStats {
            finish_reason: Some("length".to_owned()),
            ..Default::default()
        };
        assert_eq!(stop_reason(0, Some(&stats)), "max_tokens");
    }

    #[test]
    fn an_sse_frame_names_its_event_in_the_header_and_the_payload() {
        let frame = String::from_utf8(message_stop()).expect("utf8");
        assert!(frame.starts_with("event: message_stop\n"));
        assert!(frame.contains("\"type\":\"message_stop\""));
        assert!(frame.ends_with("\n\n"));
    }

    #[test]
    fn a_complete_message_carries_text_then_tool_blocks() {
        let call = ToolCall {
            id: "toolu_1".to_owned(),
            name: "read".to_owned(),
            arguments: JsonValue::Object(BTreeMap::new()),
        };
        let value = message("msg_1", "qwen3", "sure", std::slice::from_ref(&call), None);
        assert_eq!(value["content"][0]["type"], "text");
        assert_eq!(value["content"][1]["type"], "tool_use");
        assert_eq!(value["content"][1]["id"], "toolu_1");
        assert_eq!(value["stop_reason"], "tool_use");
    }

    #[test]
    fn empty_text_produces_no_text_block() {
        let value = message("msg_1", "qwen3", "", &[], None);
        assert_eq!(value["content"].as_array().map(Vec::len), Some(0));
        assert_eq!(value["stop_reason"], "end_turn");
    }
}
