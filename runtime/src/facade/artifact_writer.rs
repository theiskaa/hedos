//! [`ProvenanceArtifactWriter`]: the real [`ArtifactWriting`] sink that persists
//! a finished job's bytes into the artifact store, stamping provenance (model
//! name, resolved runtime, params, duration) looked up from the registry.

use std::sync::Arc;

use kernel::Registry;
use kernel::artifacts::{ArtifactDraft, ArtifactStore};
use kernel::jobs::Job;
use tokio::sync::Mutex;

use crate::governor::BoxFuture;
use crate::jobs::ArtifactWriting;
use crate::time::now_millis;

/// The runtime label stamped when a record has no resolved runtime yet.
const UNRESOLVED_RUNTIME: &str = "unresolved";

/// Writes a job's result into `store`, filling in provenance from the record in
/// `registry`. Both are the kernel's shared instances.
pub struct ProvenanceArtifactWriter {
    store: Arc<Mutex<ArtifactStore>>,
    registry: Arc<Mutex<Registry>>,
}

impl ProvenanceArtifactWriter {
    /// A writer sharing `store` and `registry` with the kernel that owns them.
    pub fn new(store: Arc<Mutex<ArtifactStore>>, registry: Arc<Mutex<Registry>>) -> Self {
        Self { store, registry }
    }
}

impl ArtifactWriting for ProvenanceArtifactWriter {
    fn write(
        &self,
        data: Vec<u8>,
        file_extension: String,
        job: Job,
    ) -> BoxFuture<Result<String, String>> {
        let store = Arc::clone(&self.store);
        let registry = Arc::clone(&self.registry);
        Box::pin(async move {
            let (model, runtime) = {
                let registry = registry.lock().await;
                match registry.get(&job.model_id) {
                    Some(record) => (
                        record.name.clone(),
                        record
                            .runtime
                            .id
                            .as_ref()
                            .map(|id| id.as_str().to_owned())
                            .unwrap_or_else(|| UNRESOLVED_RUNTIME.to_owned()),
                    ),
                    None => (job.model_id.clone(), UNRESOLVED_RUNTIME.to_owned()),
                }
            };
            let started_at = job.started_at.unwrap_or(job.submitted_at);
            let duration_ms = (now_millis() - started_at).max(0);
            let draft = ArtifactDraft {
                data,
                file_extension,
                preview: job.preview.clone(),
                model,
                model_id: job.model_id.clone(),
                runtime,
                capability: job.capability.clone(),
                params: job.payload.clone(),
                job_id: job.id.clone(),
                duration_ms,
                session_id: None,
            };
            let mut store = store.lock().await;
            store
                .store(draft)
                .map(|artifact| artifact.id)
                .map_err(|error| error.to_string())
        })
    }
}
