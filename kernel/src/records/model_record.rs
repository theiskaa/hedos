//! The central data model: a `ModelRecord` and the value types it composes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::records::identifiers::{
    Capability, ExecutionMode, Modality, ModelState, RunTier, RuntimeId, SourceKind,
};
use crate::records::json_value::JsonValue;
use crate::util::{hex_encode, now_millis};

/// Where a model's weights live and how it is identified.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelSource {
    /// The kind of store the model came from.
    pub kind: SourceKind,
    /// The on-disk path (or endpoint identifier) of the model.
    pub path: String,
    /// The Hugging Face / Ollama repository, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The revision/ref within the repository, when applicable.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl ModelSource {
    /// Create a source with just a kind and path.
    pub fn new(kind: SourceKind, path: &str) -> Self {
        Self {
            kind,
            path: path.to_owned(),
            repo: None,
            reference: None,
        }
    }

    /// The identity string a stable id is derived from: `kind|path|repo`.
    pub fn identity(&self) -> String {
        format!(
            "{}|{}|{}",
            self.kind.as_str(),
            self.path,
            self.repo.as_deref().unwrap_or("")
        )
    }
}

/// How a model was resolved to its runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolution {
    /// Chosen automatically by the runtime auction.
    Auto,
    /// Pinned by the user.
    User,
    /// Not yet resolved.
    #[default]
    Unresolved,
}

/// The runtime a model resolves to, plus how that choice was made.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimeRef {
    /// The winning runtime, if resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<RuntimeId>,
    /// How the runtime was chosen.
    #[serde(default)]
    pub resolved: Resolution,
    /// How much support the runtime needs.
    #[serde(default)]
    pub tier: RunTier,
    /// Other runtimes that could also serve this model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<RuntimeId>,
    /// When the user confirmed the runtime, epoch millis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<i64>,
}

/// The type of a tunable parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    /// An integer.
    Int,
    /// A floating-point number.
    Float,
    /// A boolean.
    Bool,
    /// A free string.
    String,
    /// One of a fixed set of string values.
    Enum,
}

/// The schema of one tunable parameter a model exposes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    /// The parameter's key, e.g. `temperature`.
    pub key: String,
    /// The parameter's type.
    #[serde(rename = "type")]
    pub param_type: ParamType,
    /// The default value, if any.
    #[serde(rename = "default", default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<JsonValue>,
    /// The `[min, max]` range for numeric parameters, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Vec<JsonValue>>,
    /// The allowed values for an enum parameter, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
}

/// A single model the kernel knows about: what it is, where it lives, how it runs,
/// and how the user has configured it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRecord {
    /// Stable identity derived from the source (see [`stable_id`]).
    pub id: String,
    /// The model's display name.
    pub name: String,
    /// Its primary modality.
    pub modality: Modality,
    /// Everything it can be asked to do.
    pub capabilities: Vec<Capability>,
    /// Where its weights live.
    pub source: ModelSource,
    /// The runtime it resolves to.
    #[serde(default)]
    pub runtime: RuntimeRef,
    /// The parameter schema it exposes.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// User-set parameter values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub param_values: BTreeMap<String, JsonValue>,
    /// A user-set system prompt override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// A user-set display alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// How the runtime delivers output.
    #[serde(default)]
    pub execution: ExecutionMode,
    /// Estimated memory footprint in megabytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint_mb: Option<i64>,
    /// The record's lifecycle state.
    #[serde(default)]
    pub state: ModelState,
    /// When the record was first registered, epoch millis.
    pub registered_at: i64,
    /// The primary weight file, when one file dominates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_weight_path: Option<String>,
    /// The model's context window, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<i64>,
    /// Whether the model ships a chat template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_chat_template: Option<bool>,
    /// Stop tokens the model declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_tokens: Option<Vec<String>>,
    /// Whether the model is still downloading.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub downloading: bool,
    /// A content fingerprint used to follow a model across moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_fingerprint: Option<String>,
}

impl ModelRecord {
    /// Create a record for a model, deriving its stable id from `source` and
    /// stamping `registered_at` with the current time.
    pub fn new(
        name: &str,
        modality: Modality,
        capabilities: Vec<Capability>,
        source: ModelSource,
    ) -> Self {
        Self {
            id: stable_id(&source),
            name: name.to_owned(),
            modality,
            capabilities,
            source,
            runtime: RuntimeRef::default(),
            params: Vec::new(),
            param_values: BTreeMap::new(),
            system_prompt: None,
            alias: None,
            execution: ExecutionMode::Sync,
            footprint_mb: None,
            state: ModelState::Unresolved,
            registered_at: now_millis(),
            primary_weight_path: None,
            context_length: None,
            has_chat_template: None,
            stop_tokens: None,
            downloading: false,
            content_fingerprint: None,
        }
    }

    /// The name to show the user: the alias if set, otherwise the name.
    pub fn display_name(&self) -> &str {
        match &self.alias {
            Some(alias) if !alias.is_empty() => alias,
            _ => &self.name,
        }
    }

    /// Whether the model can perform `capability`.
    pub fn can(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }
}

/// The stable identity of a model: the first eight bytes of the SHA-256 of the
/// source's `(kind, path, repo)` fields, hex-encoded. The fields are hashed with
/// a length prefix so the mapping is injective — a `|` (or any byte) inside a
/// path or repo cannot make two distinct sources collide. Two sources with the
/// same kind, path, and repo map to the same id.
pub fn stable_id(source: &ModelSource) -> String {
    let mut hasher = Sha256::new();
    for field in [
        source.kind.as_str(),
        source.path.as_str(),
        source.repo.as_deref().unwrap_or(""),
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    let digest = hasher.finalize();
    hex_encode(&digest[..8])
}
