//! Tool-calling wire types: a `ToolSpec` a model may call and a `ToolCall` it
//! emits. The `ChatMessage`-facing parsing layer (request parsing, transcript
//! inlining) lands with the chat-wire unit; this is the core the runtime
//! adapters and the tool-call chunk need.

use serde::{Deserialize, Serialize};

use crate::records::JsonValue;

/// A tool a model may be offered: a name, a description, and a JSON-Schema
/// parameter object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
}

impl ToolSpec {
    /// A tool spec.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: JsonValue,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// The `{name, description, parameters}` payload form.
    pub fn payload_value(&self) -> JsonValue {
        object([
            ("name", JsonValue::String(self.name.clone())),
            ("description", JsonValue::String(self.description.clone())),
            ("parameters", self.parameters.clone()),
        ])
    }

    /// Parse a spec from its payload form, or `None` if it has no name.
    pub fn from_payload(value: &JsonValue) -> Option<Self> {
        let fields = value.as_object()?;
        let name = fields.get("name").and_then(JsonValue::as_str)?;
        Some(Self {
            name: name.to_owned(),
            description: fields
                .get("description")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_owned(),
            parameters: fields
                .get("parameters")
                .cloned()
                .unwrap_or_else(empty_object),
        })
    }

    /// Parse an array of specs, dropping any that don't parse.
    pub fn from_payload_array(value: Option<&JsonValue>) -> Vec<Self> {
        value
            .and_then(JsonValue::as_array)
            .map(|entries| entries.iter().filter_map(Self::from_payload).collect())
            .unwrap_or_default()
    }
}

/// A tool invocation a model emitted: a call id, a tool name, and a JSON
/// arguments object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: JsonValue,
}

impl ToolCall {
    /// A call with a freshly generated id.
    pub fn new(name: impl Into<String>, arguments: JsonValue) -> Self {
        Self::with_id(new_call_id(), name, arguments)
    }

    /// A call with an explicit id.
    pub fn with_id(id: impl Into<String>, name: impl Into<String>, arguments: JsonValue) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    /// The `{id, name, arguments}` payload form.
    pub fn payload_value(&self) -> JsonValue {
        object([
            ("id", JsonValue::String(self.id.clone())),
            ("name", JsonValue::String(self.name.clone())),
            ("arguments", self.arguments.clone()),
        ])
    }

    /// Parse a call from its payload form. Requires a name and an object
    /// `arguments` (defaulting to `{}`); a fresh id is minted if none is given.
    pub fn from_payload(value: &JsonValue) -> Option<Self> {
        let fields = value.as_object()?;
        let name = fields.get("name").and_then(JsonValue::as_str)?;
        let arguments = fields
            .get("arguments")
            .cloned()
            .unwrap_or_else(empty_object);
        if !matches!(arguments, JsonValue::Object(_)) {
            return None;
        }
        let id = fields
            .get("id")
            .and_then(JsonValue::as_str)
            .filter(|id| !id.is_empty());
        Some(match id {
            Some(id) => Self::with_id(id, name, arguments),
            None => Self::new(name, arguments),
        })
    }
}

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn empty_object() -> JsonValue {
    JsonValue::Object(std::collections::BTreeMap::new())
}

fn new_call_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let entropy = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("call_{}", hex::encode(entropy.to_le_bytes()))
}
