//! The shared discovery surface: a [`StoreScanner`] inspects one on-disk model
//! store (an Ollama blob store, a Hugging Face cache, …) and returns the models
//! it found as [`DiscoveredModel`] hints, plus any per-store issues, in a
//! [`ScanResult`]. Identification later turns these hints into full records.

use crate::records::{Capability, ExecutionMode, Modality, ModelSource, SourceKind};

/// A model a scanner found on disk, as hints (not yet a resolved record). The
/// `*_hint` fields are best-effort; identification refines or overrides them.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredModel {
    /// The display name (e.g. `llama3.2:latest`).
    pub name: String,
    /// Where the weights live and what kind of store they came from.
    pub source: ModelSource,
    /// The guessed modality, if the store hinted one.
    pub modality_hint: Option<Modality>,
    /// The guessed capabilities.
    pub capabilities_hint: Vec<Capability>,
    /// How the model executes (streaming vs one-shot job).
    pub execution_hint: ExecutionMode,
    /// The on-disk footprint in bytes.
    pub footprint_bytes: i64,
    /// The primary weight file, if identified.
    pub primary_weight_path: Option<String>,
    /// Free-text notes about this specific model.
    pub diagnostics: Vec<String>,
    /// A context-window hint, if the store recorded one.
    pub context_length_hint: Option<i64>,
    /// Whether the store recorded a chat template.
    pub has_chat_template_hint: Option<bool>,
    /// Stop tokens the store recorded.
    pub stop_tokens_hint: Option<Vec<String>>,
    /// Whether the model is still downloading (weights incomplete).
    pub downloading: bool,
}

impl DiscoveredModel {
    /// A discovered model with just a name and source; hints default to empty.
    pub fn new(name: impl Into<String>, source: ModelSource) -> Self {
        Self {
            name: name.into(),
            source,
            modality_hint: None,
            capabilities_hint: Vec::new(),
            execution_hint: ExecutionMode::default(),
            footprint_bytes: 0,
            primary_weight_path: None,
            diagnostics: Vec::new(),
            context_length_hint: None,
            has_chat_template_hint: None,
            stop_tokens_hint: None,
            downloading: false,
        }
    }
}

/// The outcome of scanning one or more stores: the models found, free-text
/// issues (a store was reachable but a single entry was malformed), and the set
/// of source kinds whose scan failed wholesale (unreadable root).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanResult {
    /// The models found across the scanned stores.
    pub discovered: Vec<DiscoveredModel>,
    /// Non-fatal issues (one bad entry, an unreadable sidecar) worth surfacing.
    pub issues: Vec<String>,
    /// Source kinds whose scan failed entirely (e.g. an unreadable root).
    pub failed_kinds: Vec<SourceKind>,
}

/// A scanner over one class of on-disk model store. Synchronous: the kernel does
/// its filesystem work inline (the runtime layer decides about threads).
pub trait StoreScanner {
    /// The source kinds this scanner produces (used to mark wholesale failures).
    fn kinds(&self) -> Vec<SourceKind>;

    /// Scan the store, returning everything found plus any issues.
    fn scan(&self) -> ScanResult;
}
