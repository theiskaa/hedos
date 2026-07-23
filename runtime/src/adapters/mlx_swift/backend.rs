//! The MLX-Swift backend seam: the interface the adapter drives, and the
//! placeholder used when no in-process MLX bridge is built into this binary.

use kernel::capabilities::{ChatMessage, ToolCall, ToolSpec};

use crate::adapters::{RuntimeError, RuntimeStream};

/// One event from a running MLX-Swift generation. Unlike the Apple bridge, the
/// shim streams already-split output: visible text and reasoning arrive as
/// separate deltas (the shim owns the think-splitting and stop-matching), so
/// the adapter forwards each event through without re-processing it.
#[derive(Debug, Clone, PartialEq)]
pub enum MlxSwiftEvent {
    /// A visible-text delta.
    Text(String),
    /// A reasoning delta, already separated from the visible text.
    Thinking(String),
    /// A tool invocation the model produced.
    ToolCall(ToolCall),
    /// A human-facing progress line (loading, a no-template notice, memory
    /// pressure). Passed through as a status chunk.
    Status(String),
    /// The terminal counts and finish metadata, when the shim could measure
    /// them. The adapter times the wall-clock duration itself.
    Done(MlxSwiftDone),
}

/// The terminal metadata an MLX-Swift generation reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MlxSwiftDone {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub load_ms: Option<i64>,
    pub finish_reason: Option<String>,
    pub token_counts_estimated: bool,
}

/// The sampling knobs the MLX-Swift engine honors, mirroring the Swift
/// `MlxSwiftEngine.GenerationParams`. Only the options actually set on the
/// request are carried; unset ones fall back to the engine's own defaults.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MlxSwiftOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub repeat_penalty: Option<f64>,
    pub max_tokens: Option<i64>,
    pub stop: Vec<String>,
}

/// A stream of generation events (or a failure) a backend produces.
pub type MlxSwiftEventStream = RuntimeStream<MlxSwiftEvent>;

/// A bridge to the in-process MLX-Swift runtime.
pub trait MlxSwiftBackend: Send + Sync {
    /// Whether the in-process engine can run right now: the shim loaded and a
    /// Metal device is usable. The adapter only bids for a model when this
    /// holds, so a machine without the bridge falls through to the Python
    /// `mlx-lm` sidecar instead of resolving to a runtime that cannot serve.
    fn is_available(&self) -> bool;

    /// Stream a generation over `messages` with `options`, loading the model
    /// from `model_dir` and offering `tools` for the model to call. Dropping
    /// the returned stream cancels the generation.
    fn stream(
        &self,
        model_dir: String,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolSpec>,
        options: MlxSwiftOptions,
    ) -> MlxSwiftEventStream;
}

/// The placeholder backend used when no bridge is compiled in (or the shim did
/// not load): never available, so the adapter never bids and discovery is
/// unaffected; a generation that somehow reaches it reports the runtime as
/// unavailable.
pub struct MissingMlxSwiftBackend;

const MISSING_HINT: &str = "The in-process MLX-Swift runtime is not built into this binary; this model runs through the mlx-lm sidecar instead.";

impl MlxSwiftBackend for MissingMlxSwiftBackend {
    fn is_available(&self) -> bool {
        false
    }

    fn stream(
        &self,
        _model_dir: String,
        _messages: Vec<ChatMessage>,
        _tools: Vec<ToolSpec>,
        _options: MlxSwiftOptions,
    ) -> MlxSwiftEventStream {
        RuntimeStream::failed(RuntimeError::Unavailable(MISSING_HINT.to_owned()))
    }
}
