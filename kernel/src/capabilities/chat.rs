//! The chat-wire layer: the `ChatMessage`/`ChatAttachment` request model, the
//! lenient and strict parsers that turn an incoming OpenAI/Ollama payload into
//! messages, the outbound `payload_value` rendering, tool-spec decoding, and the
//! generic ChatML prompt fallback. This is what a gateway uses to bridge a wire
//! request to the kernel's dispatch surface and back.

use std::collections::BTreeMap;

use crate::capabilities::{ToolCall, ToolSpec};
use crate::records::JsonValue;
use crate::util::base64_encode;

const TOOL_SHAPE_HINT: &str = "each tool must be {type: \"function\", function: {name}}";
const TOOLS_ARRAY_HINT: &str = "tools must be an array of function tools";
const NO_MESSAGES_HINT: &str = "chat payload must carry a messages array or a prompt";

/// Why a chat-wire payload could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChatWireError {
    /// The payload was structurally invalid; the message explains how.
    #[error("{0}")]
    PayloadInvalid(String),
}

/// A chat message's author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatRole {
    /// The system prompt.
    System,
    /// A user turn.
    User,
    /// An assistant turn.
    Assistant,
    /// A tool result turn.
    Tool,
}

impl ChatRole {
    /// The wire string for this role.
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }

    /// The role for a wire string, if it names one.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "system" => Some(ChatRole::System),
            "user" => Some(ChatRole::User),
            "assistant" => Some(ChatRole::Assistant),
            "tool" => Some(ChatRole::Tool),
            _ => None,
        }
    }
}

/// What kind of thing a [`ChatAttachment`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentKind {
    /// An image, sent to a vision model as base64.
    Image,
    /// A document, inlined into the prompt as text.
    Document,
}

/// A file attached to a chat message: an image forwarded to a vision model, or a
/// document inlined into the prompt text.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatAttachment {
    /// Whether this is an image or a document.
    pub kind: AttachmentKind,
    /// The raw bytes.
    pub data: Vec<u8>,
    /// The MIME type.
    pub mime_type: String,
    /// An optional file name.
    pub name: Option<String>,
}

impl ChatAttachment {
    /// The prompt-inlined block for a document attachment (wrapping its UTF-8
    /// text in an `<attached-file>` element), or `None` for an image.
    pub fn inline_block(&self) -> Option<String> {
        if self.kind != AttachmentKind::Document {
            return None;
        }
        let text = String::from_utf8_lossy(&self.data);
        let open = match &self.name {
            Some(name) => format!("<attached-file name=\"{name}\">"),
            None => "<attached-file>".to_owned(),
        };
        Some(format!("{open}\n{text}\n</attached-file>"))
    }
}

/// A chat turn: role and content plus optional tool calls, tool-result routing,
/// and attachments.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// The author.
    pub role: ChatRole,
    /// The visible text content.
    pub content: String,
    /// Tool calls this (assistant) turn emitted.
    pub tool_calls: Vec<ToolCall>,
    /// The id of the tool call this (tool) turn answers.
    pub tool_call_id: Option<String>,
    /// The name of the tool this (tool) turn answers.
    pub tool_name: Option<String>,
    /// Attachments (images forwarded, documents inlined).
    pub attachments: Vec<ChatAttachment>,
    /// Content-addressed references to attachments held elsewhere.
    pub attachment_refs: Vec<String>,
}

impl ChatMessage {
    /// A plain message with just a role and content.
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            attachments: Vec::new(),
            attachment_refs: Vec::new(),
        }
    }

    /// The outbound wire object: document blocks prepended to the content, tool
    /// calls / tool routing carried through, and images base64-encoded into an
    /// `images` array.
    pub fn payload_value(&self) -> JsonValue {
        let mut parts: Vec<String> = self
            .attachments
            .iter()
            .filter_map(ChatAttachment::inline_block)
            .collect();
        parts.push(self.content.clone());
        let body = parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        let mut object = BTreeMap::new();
        object.insert(
            "role".to_owned(),
            JsonValue::String(self.role.as_str().to_owned()),
        );
        object.insert("content".to_owned(), JsonValue::String(body));
        if !self.tool_calls.is_empty() {
            object.insert(
                "tool_calls".to_owned(),
                JsonValue::Array(
                    self.tool_calls
                        .iter()
                        .map(ToolCall::payload_value)
                        .collect(),
                ),
            );
        }
        if let Some(id) = &self.tool_call_id {
            object.insert("tool_call_id".to_owned(), JsonValue::String(id.clone()));
        }
        if let Some(name) = &self.tool_name {
            object.insert("tool_name".to_owned(), JsonValue::String(name.clone()));
        }
        let images: Vec<JsonValue> = self
            .attachments
            .iter()
            .filter(|attachment| attachment.kind == AttachmentKind::Image)
            .map(|attachment| JsonValue::String(base64_encode(&attachment.data)))
            .collect();
        if !images.is_empty() {
            object.insert("images".to_owned(), JsonValue::Array(images));
        }
        JsonValue::Object(object)
    }

    /// Leniently parse one message object, returning `None` if it lacks a valid
    /// role. Unknown fields and malformed tool calls are dropped, not rejected.
    pub fn from_payload(value: &JsonValue) -> Option<Self> {
        let fields = value.as_object()?;
        let role = ChatRole::from_wire(fields.get("role")?.as_str()?)?;
        let content = fields
            .get("content")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let mut message = ChatMessage::new(role, content);
        message.apply_tool_routing(fields);
        Some(message)
    }

    /// Strictly parse the message at `index`, rejecting a non-object, a missing
    /// or unknown role, or non-string content.
    pub fn parse_strict(value: &JsonValue, index: usize) -> Result<Self, ChatWireError> {
        let Some(fields) = value.as_object() else {
            return Err(ChatWireError::PayloadInvalid(format!(
                "message at index {index} is not an object"
            )));
        };
        let role = fields
            .get("role")
            .and_then(JsonValue::as_str)
            .and_then(ChatRole::from_wire)
            .ok_or_else(|| {
                ChatWireError::PayloadInvalid(format!(
                    "message at index {index} has a missing or unknown role"
                ))
            })?;
        let content = match fields.get("content") {
            None | Some(JsonValue::Null) => "",
            Some(JsonValue::String(text)) => text,
            Some(_) => {
                return Err(ChatWireError::PayloadInvalid(format!(
                    "message at index {index} has non-string content"
                )));
            }
        };
        let mut message = ChatMessage::new(role, content);
        message.apply_tool_routing(fields);
        Ok(message)
    }

    /// Fill `tool_calls`/`tool_call_id`/`tool_name` from a message object. Shared
    /// by the lenient and strict parsers, which differ only in role/content rules.
    fn apply_tool_routing(&mut self, fields: &BTreeMap<String, JsonValue>) {
        self.tool_calls = parse_tool_calls(fields.get("tool_calls"));
        self.tool_call_id = fields
            .get("tool_call_id")
            .and_then(JsonValue::as_str)
            .map(str::to_owned);
        self.tool_name = fields
            .get("tool_name")
            .and_then(JsonValue::as_str)
            .map(str::to_owned);
    }

    /// Parse the whole chat request: a `messages` array (strictly, index by
    /// index) or, failing that, a single `prompt` string as one user turn.
    pub fn parse_all(payload: &JsonValue) -> Result<Vec<Self>, ChatWireError> {
        let fields = payload
            .as_object()
            .ok_or_else(|| ChatWireError::PayloadInvalid(NO_MESSAGES_HINT.to_owned()))?;
        if let Some(JsonValue::Array(messages)) = fields.get("messages") {
            return messages
                .iter()
                .enumerate()
                .map(|(index, value)| Self::parse_strict(value, index))
                .collect();
        }
        if let Some(JsonValue::String(prompt)) = fields.get("prompt") {
            return Ok(vec![ChatMessage::new(ChatRole::User, prompt.clone())]);
        }
        Err(ChatWireError::PayloadInvalid(NO_MESSAGES_HINT.to_owned()))
    }

    /// This message with any assistant tool calls folded into the visible text as
    /// `<tool_call>{…}</tool_call>` blocks — the transcript form for a model that
    /// takes tool history as plain text. A non-assistant or call-less message is
    /// returned unchanged.
    pub fn inlined_tool_transcript(&self) -> ChatMessage {
        if self.role != ChatRole::Assistant || self.tool_calls.is_empty() {
            return self.clone();
        }
        let mut parts = vec![self.content.clone()];
        for call in &self.tool_calls {
            let mut object = BTreeMap::new();
            object.insert("name".to_owned(), JsonValue::String(call.name.clone()));
            object.insert("arguments".to_owned(), call.arguments.clone());
            parts.push(format!(
                "<tool_call>{}</tool_call>",
                json_string(&JsonValue::Object(object))
            ));
        }
        let joined = parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        ChatMessage::new(ChatRole::Assistant, joined)
    }
}

/// Decode OpenAI-style function tools from a request's `tools` value: an array of
/// `{type: "function", function: {name, description?, parameters?}}`. Absent →
/// empty; malformed → [`ChatWireError`].
pub fn decode_tool_specs(value: Option<&JsonValue>) -> Result<Vec<ToolSpec>, ChatWireError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let JsonValue::Array(entries) = value else {
        return Err(ChatWireError::PayloadInvalid(TOOLS_ARRAY_HINT.to_owned()));
    };
    // Swift casts the whole array to `[[String: Any]]`, so a non-object element
    // fails the array shape wholesale (not the per-tool shape). Match that.
    if entries.iter().any(|entry| entry.as_object().is_none()) {
        return Err(ChatWireError::PayloadInvalid(TOOLS_ARRAY_HINT.to_owned()));
    }
    let mut specs = Vec::with_capacity(entries.len());
    for entry in entries {
        let object = entry.as_object();
        let kind = object
            .and_then(|fields| fields.get("type"))
            .and_then(JsonValue::as_str)
            .unwrap_or("function");
        let function = object
            .and_then(|fields| fields.get("function"))
            .and_then(JsonValue::as_object);
        let name = function
            .and_then(|fields| fields.get("name"))
            .and_then(JsonValue::as_str)
            .filter(|name| !name.is_empty());
        let (Some(function), Some(name)) = (function, name) else {
            return Err(ChatWireError::PayloadInvalid(TOOL_SHAPE_HINT.to_owned()));
        };
        if kind != "function" {
            return Err(ChatWireError::PayloadInvalid(TOOL_SHAPE_HINT.to_owned()));
        }
        let description = function
            .get("description")
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_owned();
        let parameters = match function.get("parameters") {
            Some(JsonValue::Object(fields)) => JsonValue::Object(fields.clone()),
            _ => JsonValue::Object(BTreeMap::new()),
        };
        specs.push(ToolSpec::new(name, description, parameters));
    }
    Ok(specs)
}

/// The generic ChatML fallback prompt for a model with no chat template.
pub struct ChatMlPrompt;

impl ChatMlPrompt {
    /// Shown when a model declares no chat template and this format is used.
    pub const NO_TEMPLATE_NOTICE: &'static str =
        "this model has no chat template — using a generic format";

    /// Render `messages` as a ChatML prompt ending with an open assistant turn.
    pub fn render(messages: &[ChatMessage]) -> String {
        let mut prompt = String::new();
        for message in messages {
            prompt.push_str("<|im_start|>");
            prompt.push_str(message.role.as_str());
            prompt.push('\n');
            prompt.push_str(&message.content);
            prompt.push_str("<|im_end|>\n");
        }
        prompt.push_str("<|im_start|>assistant\n");
        prompt
    }
}

fn parse_tool_calls(value: Option<&JsonValue>) -> Vec<ToolCall> {
    match value {
        Some(JsonValue::Array(calls)) => calls.iter().filter_map(ToolCall::from_payload).collect(),
        _ => Vec::new(),
    }
}

/// Serialize `value` to a compact JSON string with sorted keys. `JsonValue`'s
/// object is a `BTreeMap`, so serialization is already key-sorted.
fn json_string(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned())
}
