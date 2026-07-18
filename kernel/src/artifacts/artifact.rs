//! The stored `Artifact` record and the `ArtifactDraft` a writer submits.

use serde::{Deserialize, Serialize};

use crate::records::{Capability, JsonValue};

/// A stored generated output: where its bytes live, a content hash, provenance
/// (model/runtime/capability/params), timing, and ownership (job, session).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    /// The output file path, relative to the store root.
    pub path: String,
    /// SHA-256 (hex) of the output bytes.
    pub content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preview_path: Option<String>,
    /// The model's display name.
    pub model: String,
    /// The model's registry id.
    pub model_id: String,
    pub runtime: String,
    pub capability: Capability,
    pub params: JsonValue,
    /// Creation time, epoch milliseconds.
    pub created_at: i64,
    pub duration_ms: i64,
    pub job_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_id: Option<String>,
}

/// Everything needed to store one output. The store fills in the id, path,
/// content hash, preview path, and creation time.
#[derive(Debug, Clone)]
pub struct ArtifactDraft {
    pub data: Vec<u8>,
    pub file_extension: String,
    pub preview: Option<Vec<u8>>,
    pub model: String,
    pub model_id: String,
    pub runtime: String,
    pub capability: Capability,
    pub params: JsonValue,
    pub job_id: String,
    pub duration_ms: i64,
    pub session_id: Option<String>,
}
