//! The two event vocabularies: what a job runner streams (`JobRuntimeEvent`) and
//! what the scheduler publishes to observers (`JobEvent`).

use super::job::JobProgress;

/// One item a job runtime (image generation) streams back. The sidecar job pump
/// emits `Started`/`Progress`/`Preview`/`Result`; `Status` and `Artifacts` are
/// added by the scheduler layer that wraps the pump.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JobRuntimeEvent {
    /// A free-text status notice (emitted by the scheduler, not the pump).
    Status(String),
    /// Generation has begun.
    Started,
    /// Progress through a fixed number of steps.
    Progress { step: i64, total_steps: i64 },
    /// An intermediate preview image.
    Preview(Vec<u8>),
    /// A finished output artifact and the file extension it should carry.
    Result {
        data: Vec<u8>,
        file_extension: String,
    },
    /// The set of artifact paths a job produced (emitted by the scheduler).
    Artifacts(Vec<String>),
}

/// What the scheduler publishes to `events(id)` observers. Coarser than the
/// runtime stream: it collapses the runner's ticks into the observable state.
#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    /// The job is queued, optionally with a human reason for the wait.
    Queued { reason: Option<String> },
    /// The job has passed admission and is preparing.
    Preparing,
    /// A free-text status notice.
    Status(String),
    /// The job is actively running.
    Running,
    /// Monotone progress.
    Progress(JobProgress),
    /// An intermediate preview image.
    Preview(Vec<u8>),
    /// The job finished, with its result artifact ids.
    Done { result: Vec<String> },
    /// The job failed, with a message.
    Failed { message: String },
    /// The job was cancelled.
    Cancelled,
}
