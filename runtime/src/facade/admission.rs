//! [`GovernorAdmission`]: the real [`JobAdmission`] that gates a job on the
//! memory governor. The scheduler was built against the `JobAdmission` trait
//! with an `ImmediateAdmission` stand-in; this bridges it to the governor so
//! queued jobs wait for RAM the way streaming requests do.

use std::sync::Arc;

use kernel::Registry;
use kernel::jobs::Job;
use tokio::sync::Mutex;

use crate::governor::{BoxFuture, MemoryGovernor, RamVerdict};
use crate::jobs::{JobAdmission, JobError, OnWait};

/// Admits jobs through the memory governor, looking the model's footprint up in
/// the registry so eviction accounts for the incoming weights.
pub struct GovernorAdmission {
    governor: Arc<MemoryGovernor>,
    registry: Arc<Mutex<Registry>>,
}

impl GovernorAdmission {
    /// A governor-backed admission sharing `governor` and `registry` with the
    /// kernel that owns them.
    pub fn new(governor: Arc<MemoryGovernor>, registry: Arc<Mutex<Registry>>) -> Self {
        Self { governor, registry }
    }
}

impl JobAdmission for GovernorAdmission {
    fn admit(&self, job: Job, on_wait: OnWait) -> BoxFuture<Result<RamVerdict, JobError>> {
        let governor = Arc::clone(&self.governor);
        let registry = Arc::clone(&self.registry);
        Box::pin(async move {
            let (name, footprint) = {
                let registry = registry.lock().await;
                match registry.get(&job.model_id) {
                    Some(record) => (record.name.clone(), record.footprint_mb),
                    None => (job.model_id.clone(), None),
                }
            };
            // The governor reports a wait synchronously; the scheduler's `on_wait`
            // is async, so fire it off (it only updates the job's queue reason —
            // advisory state that need not settle before admission proceeds).
            let report = move |reason: &str| {
                tokio::spawn(on_wait(reason.to_owned()));
            };
            let verdict = governor
                .admit(&job.model_id, &name, footprint, Some(&report))
                .await;
            Ok(verdict)
        })
    }
}
