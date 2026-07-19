//! The sidecar machinery: managed Python child processes spoken to over a
//! length-prefixed frame protocol. [`SidecarSupervisor`] owns process lifecycle,
//! the ready handshake, exclusive per-process sessions, progress/cancel
//! watchdogs, and the two pumps that turn frames into [`CapabilityChunk`] and
//! [`JobRuntimeEvent`] streams.
//!
//! Like the governor, each Swift `actor` becomes a struct whose synchronous
//! state lives behind a `std::sync::Mutex` that is **never held across an
//! `.await`** — reader/writer/watchdog tasks and pumps lock, mutate, and drop
//! the guard before awaiting.

mod bundle;
mod model_paths;
mod spec;
mod supervisor;

pub use bundle::{RuntimeBundle, SidecarWorkdir, spec as bundle_spec};
pub use model_paths::SidecarModelPaths;
pub use spec::{DEFAULT_SAMPLE_RATE, SidecarSpec};
pub use supervisor::SidecarSupervisor;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use tokio::sync::mpsc;

/// Errors from a sidecar request or spawn.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SidecarError {
    /// The sidecar process died or never started.
    #[error("sidecar {runtime_id} {detail}")]
    SidecarDied { runtime_id: String, detail: String },
    /// The runtime reported a failure, or a wire/encode step failed.
    #[error("{0}")]
    RuntimeFailed(String),
    /// The request was cancelled (the sidecar acknowledged a `cancel`).
    #[error("cancelled")]
    Cancelled,
}

/// A stream of a request's results. Consume with [`recv`](Self::recv) until it
/// returns `None`. Dropping the stream cancels the request — cooperatively (a
/// `cancel` op, keeping the sidecar warm) or by killing the process, per the
/// spec's `cooperative_cancel` flag (job requests always cancel cooperatively).
pub struct SidecarStream<T> {
    pub(crate) rx: mpsc::UnboundedReceiver<Result<T, SidecarError>>,
}

impl<T> SidecarStream<T> {
    /// The next result, or `None` when the stream is exhausted.
    pub async fn recv(&mut self) -> Option<Result<T, SidecarError>> {
        self.rx.recv().await
    }
}

/// The result type carried by a capability (chat/vision/speech/embed) stream.
pub type ChunkStream = SidecarStream<CapabilityChunk>;

/// The result type carried by a job (image generation) stream.
pub type JobStream = SidecarStream<JobRuntimeEvent>;
