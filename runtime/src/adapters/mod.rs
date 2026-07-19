//! The runtime adapter interface: how the kernel drives a model through some
//! concrete backend (a Python sidecar, a local daemon over HTTP, a manifest
//! command, …). Every adapter turns a `ModelRecord` + a request into a uniform
//! `CapabilityChunk` (or `JobRuntimeEvent`) stream, so the layers above don't
//! care what runs underneath.
//!
//! Each adapter offers a [`RuntimeAdapter::bid`] for a model at identification
//! time; the ranking engine that runs the auction over those bids lives in the
//! resolution unit ([`crate::resolution`]). The invoke path selects an adapter by
//! [`RuntimeAdapter::can_serve`].

mod grammar;
mod llama_pool;
mod llama_server;
mod mlx_lm;
mod mlx_vlm;
mod ollama;
mod openai;
mod sidecar_stream;
mod tool_scanner;

pub use grammar::{
    CALL_CLOSE, CALL_OPEN, GENERIC_JSON_GRAMMAR, ToolGrammarError, grammar_for_response_format,
    tool_grammar, tool_system_block,
};
pub use llama_pool::{LlamaServerPool, LlamaServerSpawner, ServerProcess, ServerSpawner};
pub use llama_server::{BackendFuture, LlamaBackend, LlamaServerAdapter};
pub use mlx_lm::MlxLmAdapter;
pub use mlx_vlm::MlxVlmAdapter;
pub use ollama::OllamaAdapter;
pub use openai::{EndpointConcurrencyGate, EnvSecretStore, OpenAiEndpointAdapter, SecretStore};
pub use tool_scanner::ToolCallScanner;

use std::collections::HashSet;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use kernel::records::{Capability, JsonValue, ModelRecord, RuntimeId};
use kernel::resolution::{IdentifiedModel, RuntimeBid};
use tokio::sync::mpsc;

use crate::sidecar::SidecarError;

/// Why an adapter request ended: a cooperative cancel, a failure, an unavailable
/// runtime, or a job-only runner asked to stream.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeError {
    /// The request was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// The runtime reported a failure.
    #[error("{0}")]
    Failed(String),
    /// The runtime is not available (missing binary, daemon down, …).
    #[error("{0}")]
    Unavailable(String),
    /// A job-only runner's streaming `invoke` was called — the dispatch guard a
    /// job runner returns to reject the streaming path (it only runs jobs).
    #[error("this runtime runs jobs, not streaming requests")]
    WrongExecutionMode,
}

impl From<SidecarError> for RuntimeError {
    fn from(error: SidecarError) -> Self {
        match error {
            SidecarError::Cancelled => RuntimeError::Cancelled,
            SidecarError::RuntimeFailed(message) => RuntimeError::Failed(message),
            // Keep the full "sidecar <id> <detail>" message — the id is the most
            // useful part of the diagnostic.
            died @ SidecarError::SidecarDied { .. } => RuntimeError::Failed(died.to_string()),
        }
    }
}

/// A stream of an adapter request's results. Consume with [`recv`](Self::recv)
/// until it returns `None`. Dropping the stream closes the channel; adapters
/// observe the closed sender and cancel their work (the actual cancel is the
/// adapter's contract, not enforced by this type).
pub struct RuntimeStream<T> {
    rx: mpsc::UnboundedReceiver<Result<T, RuntimeError>>,
}

impl<T> RuntimeStream<T> {
    /// A stream and the sender an adapter feeds it through.
    pub fn channel() -> (mpsc::UnboundedSender<Result<T, RuntimeError>>, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (tx, Self { rx })
    }

    /// A stream that immediately yields one error and ends (for the
    /// reject-early paths like `can_serve` mismatches).
    pub fn failed(error: RuntimeError) -> Self {
        let (tx, stream) = Self::channel();
        let _ = tx.send(Err(error));
        stream
    }

    /// The next result, or `None` when the stream is exhausted.
    pub async fn recv(&mut self) -> Option<Result<T, RuntimeError>> {
        self.rx.recv().await
    }
}

/// A capability (chat/vision/speech/transcription/embeddings) result stream.
pub type ChunkStream = RuntimeStream<CapabilityChunk>;

/// A job (image generation) result stream.
pub type JobStream = RuntimeStream<JobRuntimeEvent>;

/// A backend that can serve models through the streaming `invoke` path.
pub trait RuntimeAdapter: Send + Sync {
    /// The adapter's runtime id.
    fn id(&self) -> &RuntimeId;

    /// Whether this adapter can serve `capability` for `record`.
    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool;

    /// This adapter's offer to serve `record` given its [`IdentifiedModel`], or
    /// `None` if it cannot run it. The resolution auction ranks these bids to
    /// pick the winning runtime. Adapters that never win a model unprompted (the
    /// endpoint/manifest kinds) leave the default `None`.
    fn bid(&self, _record: &ModelRecord, _identified: &IdentifiedModel) -> Option<RuntimeBid> {
        None
    }

    /// Serve a streaming request, yielding capability chunks. `capability` is
    /// taken by value so the adapter can move it into its feeder task; `can_serve`
    /// borrows it because it is a pure predicate run over every candidate adapter.
    fn invoke(
        &self,
        record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream;

    /// The effective context window for `record` given a `requested` size, if the
    /// adapter constrains it.
    fn effective_context_window(
        &self,
        _record: &ModelRecord,
        _requested: Option<i64>,
    ) -> Option<i64> {
        None
    }

    /// Whether the adapter supports tool calls for `record`.
    fn supports_tools(&self, _record: &ModelRecord) -> bool {
        false
    }

    /// The parameter keys the adapter actually honors for a capability (others
    /// are dropped from the request).
    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        _capability: &Capability,
    ) -> HashSet<String> {
        HashSet::new()
    }
}

/// A backend that runs jobs (image generation) rather than streaming requests.
pub trait JobRunning: Send + Sync {
    /// Run a job, yielding job runtime events.
    fn run(&self, record: &ModelRecord, capability: Capability, payload: JsonValue) -> JobStream;
}
