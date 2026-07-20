//! The local llama.cpp adapter, served through `llama-server` rather than an
//! in-process FFI engine. `llama-server` speaks the OpenAI API, so this adapter
//! ensures a server is running for the model's GGUF, then proxies the request
//! through the shared OpenAI streaming path.
//!
//! Rather than running llama.cpp in-process over a Metal FFI binding, this takes
//! the subprocess route and avoids the FFI: the non-streaming surface
//! (bid/can_serve/context/honored params) lives here, and the engine is a
//! [`LlamaBackend`] that hands back a running server's base URL.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

use super::openai::{OPTION_KEYS, request_body, stream_completions};
use super::{ChunkStream, RuntimeAdapter, RuntimeError};

const DEFAULT_CONTEXT: i64 = 4096;
const MAX_DEFAULT_CONTEXT: i64 = 32768;
const MIN_CONTEXT: i64 = 512;

/// A future returning a running server's OpenAI base URL (or why it couldn't be
/// started).
pub type BackendFuture = Pin<Box<dyn Future<Output = Result<String, RuntimeError>> + Send>>;

/// Ensures a local OpenAI-compatible server is running for a model and returns its
/// base URL. The production implementation spawns and supervises `llama-server`
/// (a separable unit); this trait is the seam the adapter drives, so it can run
/// against any such server — real or mocked.
pub trait LlamaBackend: Send + Sync {
    /// Ensure a server is running for `record` sized to `context_tokens`, and
    /// return its base URL (e.g. `http://127.0.0.1:8080`).
    fn base_url(&self, record: &ModelRecord, context_tokens: i64) -> BackendFuture;
}

/// Serves local GGUF models by proxying to a `llama-server` instance.
pub struct LlamaServerAdapter {
    id: RuntimeId,
    backend: Arc<dyn LlamaBackend>,
    client: reqwest::Client,
}

impl LlamaServerAdapter {
    /// An adapter over `backend`.
    pub fn new(backend: Arc<dyn LlamaBackend>) -> Self {
        Self {
            id: RuntimeId::llama_cpp(),
            backend,
            client: reqwest::Client::new(),
        }
    }

    /// The effective context window: the requested size (or a capped default)
    /// clamped into `[min(512, base), base]`, where `base` is the model's declared
    /// context length or 4096.
    pub fn effective_context_tokens(record: &ModelRecord, requested: Option<i64>) -> i64 {
        let base = record
            .context_length
            .filter(|length| *length > 0)
            .unwrap_or(DEFAULT_CONTEXT);
        let capped_default = base.min(MAX_DEFAULT_CONTEXT);
        let lower = base.min(MIN_CONTEXT);
        requested.unwrap_or(capped_default).max(lower).min(base)
    }
}

impl RuntimeAdapter for LlamaServerAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        let served = *capability == Capability::chat() || *capability == Capability::complete();
        served && record.runtime.id.as_ref() == Some(&self.id)
    }

    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::Gguf
            || !identified.capabilities.contains(&Capability::chat())
        {
            return None;
        }
        Some(RuntimeBid::new(RunTier::Native, BidPreference::LLAMA_CPP))
    }

    fn effective_context_window(
        &self,
        record: &ModelRecord,
        requested: Option<i64>,
    ) -> Option<i64> {
        Some(Self::effective_context_tokens(record, requested))
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let requested = payload
            .as_object()
            .and_then(|fields| fields.get("context_length"))
            .and_then(JsonValue::as_i64);
        let context_tokens = Self::effective_context_tokens(record, requested);
        let model = wire_model_name(record);
        let backend = Arc::clone(&self.backend);
        let client = self.client.clone();
        let record = record.clone();

        tokio::spawn(async move {
            // Race the (potentially slow) server startup against a consumer drop,
            // so cancelling mid-spawn doesn't keep a cold `llama-server` booting for
            // a result nobody will read.
            let base = tokio::select! {
                result = backend.base_url(&record, context_tokens) => match result {
                    Ok(base) => base,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                },
                _ = tx.closed() => return,
            };
            let body = match request_body(&model, &payload) {
                Ok(body) => body,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            // Local server → no API key.
            stream_completions(&client, &base, None, body, &tx).await;
        });
        stream
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        // Derived from the OpenAI forward set so the two can't drift, plus
        // `context_length` (consumed at server-spawn as `--ctx-size`, not in the
        // request body). The llama-specific extras (top_k/min_p/repeat_penalty) are
        // deferred with a per-request llama body.
        OPTION_KEYS
            .iter()
            .copied()
            .chain(["context_length"])
            .map(str::to_owned)
            .collect()
    }
}

/// The wire model name `llama-server` is asked for. The server serves whatever
/// GGUF it loaded regardless, so this is only a label; a repo id is preferred over
/// the display name.
fn wire_model_name(record: &ModelRecord) -> String {
    record
        .source
        .repo
        .clone()
        .unwrap_or_else(|| record.name.clone())
}

/// The GGUF path a `llama-server` is launched with: the primary weight file when
/// one dominates, else the source path. Returned as stored (no `~` expansion —
/// discovery passes absolute paths).
pub(crate) fn model_gguf_path(record: &ModelRecord) -> &str {
    record
        .primary_weight_path
        .as_deref()
        .unwrap_or(&record.source.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn record() -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::file(), "/w.gguf"),
        )
    }

    #[test]
    fn wire_model_name_prefers_the_repo() {
        let mut rec = record();
        assert_eq!(wire_model_name(&rec), "m");
        rec.source.repo = Some("org/model".to_owned());
        assert_eq!(wire_model_name(&rec), "org/model");
    }

    #[test]
    fn effective_context_treats_nonpositive_declared_length_as_the_default() {
        let mut rec = record();
        rec.context_length = Some(0);
        assert_eq!(
            LlamaServerAdapter::effective_context_tokens(&rec, None),
            4096
        );
        rec.context_length = Some(-5);
        assert_eq!(
            LlamaServerAdapter::effective_context_tokens(&rec, None),
            4096
        );
    }
}
