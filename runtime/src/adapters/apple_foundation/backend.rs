//! The Apple-Foundation backend seam: the interface the adapter drives, and the
//! placeholder used when no bridge to Apple's model is built into this binary.

use kernel::capabilities::{ChatMessage, ToolCall, ToolSpec};

use crate::adapters::{RuntimeError, RuntimeStream};

/// Whether Apple's on-device model can serve right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinAvailability {
    /// The model is ready to answer.
    Available,
    /// Apple Intelligence is switched off in System Settings.
    NotEnabled,
    /// The model is still downloading.
    NotReady,
    /// This device or OS cannot run Apple's model.
    NotEligible,
}

/// One event from a running generation. The model re-emits the whole reply on
/// each step, so text arrives as cumulative snapshots, not deltas.
#[derive(Debug, Clone, PartialEq)]
pub enum BuiltinEvent {
    /// The full reply text so far.
    Snapshot(String),
    /// A tool invocation the model captured mid-generation. The turn still
    /// ends with a `Done` — the backend finishes the generation once a call
    /// is captured, since the tool's result arrives in a later request.
    ToolCall(ToolCall),
    /// The terminal token counts, when the backend could measure them.
    Done {
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
    },
}

/// The sampling knobs Apple's model honors.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BuiltinOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<i64>,
    pub seed: Option<u64>,
    pub max_tokens: Option<i64>,
}

/// A stream of generation events (or a failure) a backend produces.
pub type BuiltinEventStream = RuntimeStream<BuiltinEvent>;

/// A bridge to Apple's on-device model (Apple Intelligence).
pub trait AppleFoundationBackend: Send + Sync {
    /// Whether the model can serve right now.
    fn availability(&self) -> BuiltinAvailability;

    /// Stream a generation over `messages` with `options`, offering `tools`
    /// for the model to call. Dropping the returned stream cancels the
    /// generation.
    fn stream(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<ToolSpec>,
        options: BuiltinOptions,
    ) -> BuiltinEventStream;
}

/// The placeholder backend used when no bridge is compiled in: the model is
/// never available (so discovery stays silent) and a generation that somehow
/// reaches it reports the runtime as unavailable.
pub struct MissingAppleBackend;

const MISSING_HINT: &str =
    "Apple's model needs the Apple Intelligence bridge, which is not built into this binary.";

impl AppleFoundationBackend for MissingAppleBackend {
    fn availability(&self) -> BuiltinAvailability {
        BuiltinAvailability::NotEligible
    }

    fn stream(
        &self,
        _messages: Vec<ChatMessage>,
        _tools: Vec<ToolSpec>,
        _options: BuiltinOptions,
    ) -> BuiltinEventStream {
        RuntimeStream::failed(RuntimeError::Unavailable(MISSING_HINT.to_owned()))
    }
}
