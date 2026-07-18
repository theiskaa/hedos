//! The async job scheduler: runs discrete units of work one at a time, against
//! an injected admission authority, artifact sink, and per-job runner. The pure
//! data model (`Job`/`JobEvent`/`JobHistoryStore`/seeding) lives in the kernel;
//! this crate drives it.
//!
//! Modeled on the governor/sidecar discipline: one `Mutex<State>` that is never
//! held across an `.await`.

mod scheduler;

pub use scheduler::JobScheduler;

use std::sync::Arc;

use kernel::jobs::Job;
use tokio::sync::mpsc;

use crate::governor::{BoxFuture, RamVerdict};

/// Why a job ended, as reported by admission or a runner: a cooperative cancel,
/// or a failure carrying a message.
#[derive(Debug, Clone)]
pub enum JobError {
    /// The work was cancelled.
    Cancelled,
    /// The work failed with this message.
    Failed(String),
}

/// The stream a runner produces: job runtime events until it ends, or a
/// [`JobError`] that concludes the job.
pub type RunnerStream = mpsc::UnboundedReceiver<Result<kernel::jobs::JobRuntimeEvent, JobError>>;

/// Builds the runner stream for one job execution. Called at most once, when the
/// job reaches the front of the queue.
pub type Runner = Box<dyn FnOnce() -> RunnerStream + Send>;

/// A callback a [`JobAdmission`] invokes to report why it is making a job wait.
pub type OnWait = Arc<dyn Fn(String) -> BoxFuture<()> + Send + Sync>;

/// Decides whether a job may start, possibly waiting (and reporting the wait
/// through `on_wait`) until memory is available.
pub trait JobAdmission: Send + Sync {
    /// Admit `job`, returning the advisory RAM verdict, or a [`JobError`] if the
    /// wait was cancelled or admission failed.
    fn admit(&self, job: Job, on_wait: OnWait) -> BoxFuture<Result<RamVerdict, JobError>>;
}

/// Admission that always lets a job run immediately with an `Ok` verdict.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImmediateAdmission;

impl JobAdmission for ImmediateAdmission {
    fn admit(&self, _job: Job, _on_wait: OnWait) -> BoxFuture<Result<RamVerdict, JobError>> {
        Box::pin(async { Ok(RamVerdict::Ok) })
    }
}

/// Persists a job's `.result` bytes into an artifact store, returning the new
/// artifact id (or an error message).
pub trait ArtifactWriting: Send + Sync {
    /// Write `data` (with `file_extension`) as an artifact owned by `job`.
    fn write(
        &self,
        data: Vec<u8>,
        file_extension: String,
        job: Job,
    ) -> BoxFuture<Result<String, String>>;
}
