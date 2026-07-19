//! The [`Kernel`] facade: the runtime's dependency-injection entry point. It
//! owns the registry, governor, job scheduler, artifact store, and the adapter
//! list, and exposes the surface the gateway/cli drive — `invoke` (streaming),
//! `submit`/`rerun`/`vary` (jobs), and the capability queries around them.
//!
//! It also wires the two pieces the scheduler was built to accept but the job
//! unit deferred: [`GovernorAdmission`] (jobs wait on the governor for RAM) and
//! [`ProvenanceArtifactWriter`] (job results land in the store with provenance).

mod admission;
mod artifact_writer;

pub use admission::GovernorAdmission;
pub use artifact_writer::ProvenanceArtifactWriter;

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use kernel::artifacts::{Artifact, ArtifactStore};
use kernel::discovery::{DiscoveryService, DiscoverySummary, StoreScanner};
use kernel::jobs::{JobHistoryStore, reseeded, seeded};
use kernel::profiles::{Verdict, assess, merged, prompt_characters};
use kernel::records::{Capability, JsonValue, ModelRecord, SourceKind};
use kernel::{Registry, RegistryError};
use tokio::sync::{Mutex, mpsc};

use crate::adapters::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError};
use crate::governor::MemoryGovernor;
use crate::jobs::{JobError, JobScheduler, Runner, RunnerStream};
use crate::resolution::{ResolutionEngine, ResolutionExplanation};

/// A model window's implicit completion length when the caller sets no
/// `max_tokens`. A clamp at or above this is only written back when the caller
/// asked for a specific `max_tokens`; below it the window itself is the limit,
/// so the clamp is always applied. Mirrors the Swift kernel's threshold.
const IMPLICIT_MAX_TOKENS: i64 = 4096;

/// A builtin model's fixed context window.
const BUILTIN_WINDOW: i64 = 4096;

/// Why a kernel request could not be served.
#[derive(Debug, Clone, thiserror::Error)]
pub enum KernelError {
    /// No model is registered under this id.
    #[error("no model with id {0} is registered")]
    ModelNotFound(String),
    /// No artifact is stored under this id.
    #[error("no artifact with id {0} is stored")]
    ArtifactNotFound(String),
    /// No adapter serves this capability for the model.
    #[error("{model} has no runtime for {capability}")]
    CapabilityUnsupported {
        /// The model's name.
        model: String,
        /// The capability that has no runtime.
        capability: Capability,
    },
    /// The prompt no longer fits the model's context window.
    #[error("this conversation no longer fits {model}'s context window")]
    ContextExceeded {
        /// The model's name.
        model: String,
    },
    /// The request payload was rejected before dispatch.
    #[error("{0}")]
    PayloadInvalid(String),
    /// The runtime could not serve the request as asked.
    #[error("{0}")]
    RuntimeFailed(String),
    /// The artifact store failed.
    #[error("{0}")]
    Storage(String),
}

impl From<RegistryError> for KernelError {
    fn from(error: RegistryError) -> Self {
        KernelError::Storage(error.to_string())
    }
}

/// A resident model as reported to callers: the governor's accounting of what
/// currently holds memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResidentEntry {
    /// The model id, or `None` for a resident held outside the governor's
    /// accounting (a model loaded directly by the Ollama daemon). Only governor
    /// residents are reported today; the Ollama-origin source is deferred with
    /// the adapter's loaded-model tracking, and will fill the `None` case.
    pub model_id: Option<String>,
    /// The model's display name.
    pub name: String,
    /// The footprint in megabytes.
    pub footprint_mb: i64,
}

/// An adapter registered with the kernel, plus its job-running handle when the
/// same backend also runs jobs (image generation). The streaming `invoke` path
/// uses `adapter`; the `submit` path requires `job`.
pub struct RegisteredAdapter {
    adapter: Arc<dyn RuntimeAdapter>,
    job: Option<Arc<dyn JobRunning>>,
}

impl RegisteredAdapter {
    /// A streaming-only adapter (no job path).
    pub fn streaming(adapter: Arc<dyn RuntimeAdapter>) -> Self {
        Self { adapter, job: None }
    }

    /// An adapter that also runs jobs. `adapter` and `job` are the same backend
    /// under both trait objects.
    pub fn with_jobs(adapter: Arc<dyn RuntimeAdapter>, job: Arc<dyn JobRunning>) -> Self {
        Self {
            adapter,
            job: Some(job),
        }
    }
}

/// The runtime entry point: resolves a request to an adapter, applies the shared
/// prompt/param/context policy, and drives it through the governor-backed
/// scheduler (jobs) or straight to the adapter (streams).
pub struct Kernel {
    registry: Arc<Mutex<Registry>>,
    artifacts: Arc<Mutex<ArtifactStore>>,
    governor: Arc<MemoryGovernor>,
    scheduler: Arc<JobScheduler>,
    adapters: Vec<RegisteredAdapter>,
    default_prompt: StdMutex<Option<String>>,
}

impl Kernel {
    /// Wire a kernel over its owned subsystems. The scheduler is built here with
    /// the governor-backed admission and the provenance artifact writer, sharing
    /// `registry`/`artifacts` with the caller-facing paths.
    pub fn new(
        registry: Registry,
        artifacts: ArtifactStore,
        governor: Arc<MemoryGovernor>,
        history: JobHistoryStore,
        adapters: Vec<RegisteredAdapter>,
    ) -> Self {
        let registry = Arc::new(Mutex::new(registry));
        let artifacts = Arc::new(Mutex::new(artifacts));
        let admission = Arc::new(GovernorAdmission::new(
            Arc::clone(&governor),
            Arc::clone(&registry),
        ));
        let writer = Arc::new(ProvenanceArtifactWriter::new(
            Arc::clone(&artifacts),
            Arc::clone(&registry),
        ));
        let scheduler = Arc::new(JobScheduler::new(history, admission, Some(writer)));
        Self {
            registry,
            artifacts,
            governor,
            scheduler,
            adapters,
            default_prompt: StdMutex::new(None),
        }
    }

    /// The job scheduler, for callers that poll or subscribe to job state.
    pub fn scheduler(&self) -> &JobScheduler {
        &self.scheduler
    }

    /// The memory governor.
    pub fn governor(&self) -> &MemoryGovernor {
        &self.governor
    }

    /// Set the fallback chat system prompt applied when neither the record nor
    /// the session carries one. (Stands in for the not-yet-ported settings
    /// store, whose `defaultSystemPrompt` the Swift kernel reads here.)
    pub fn set_default_system_prompt(&self, prompt: Option<String>) {
        if let Ok(mut slot) = self.default_prompt.lock() {
            *slot = prompt;
        }
    }

    fn default_system_prompt(&self) -> Option<String> {
        self.default_prompt
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// The fallback system prompt for a chat request (none for other
    /// capabilities). The shared prompt policy both dispatch paths apply.
    fn chat_fallback(&self, capability: &Capability) -> Option<String> {
        if *capability == Capability::chat() {
            self.default_system_prompt()
        } else {
            None
        }
    }

    async fn record(&self, model_id: &str) -> Result<ModelRecord, KernelError> {
        self.registry
            .lock()
            .await
            .get(model_id)
            .cloned()
            .ok_or_else(|| KernelError::ModelNotFound(model_id.to_owned()))
    }

    fn adapter_for(
        &self,
        record: &ModelRecord,
        capability: &Capability,
    ) -> Result<&RegisteredAdapter, KernelError> {
        self.adapters
            .iter()
            .find(|entry| entry.adapter.can_serve(record, capability))
            .ok_or_else(|| KernelError::CapabilityUnsupported {
                model: record.name.clone(),
                capability: capability.clone(),
            })
    }

    /// Open a streaming request: resolve the model, pick an adapter, merge the
    /// record's params and system prompt into the payload, clamp it to the
    /// context window, and hand off to the adapter.
    pub async fn invoke(
        &self,
        model_id: &str,
        capability: Capability,
        payload: JsonValue,
    ) -> Result<ChunkStream, KernelError> {
        self.invoke_with(model_id, capability, payload, None, None)
            .await
    }

    /// [`invoke`](Self::invoke) with an explicit session system-prompt override
    /// and an appended prompt block (e.g. a tool preamble).
    pub async fn invoke_with(
        &self,
        model_id: &str,
        capability: Capability,
        payload: JsonValue,
        system_prompt_override: Option<&str>,
        prompt_suffix: Option<&str>,
    ) -> Result<ChunkStream, KernelError> {
        let record = self.record(model_id).await?;
        let entry = self.adapter_for(&record, &capability)?;
        let adapter = entry.adapter.as_ref();

        if payload_carries_images(&payload) && !adapter.can_serve(&record, &Capability::see()) {
            return Err(KernelError::PayloadInvalid(format!(
                "{} cannot read images; this runtime has no vision path.",
                record.name
            )));
        }

        let fallback = self.chat_fallback(&capability);
        let configured = merged(
            &record,
            &capability,
            payload,
            fallback.as_deref(),
            system_prompt_override,
            prompt_suffix,
        );
        let configured = if capability == Capability::chat() || capability == Capability::complete()
        {
            clamp_to_window(&record, adapter, configured)?
        } else {
            configured
        };

        Ok(adapter.invoke(&record, capability, configured))
    }

    /// Queue a job: resolve the model, require a job-running adapter, merge and
    /// seed the payload, and submit it to the governor-backed scheduler.
    pub async fn submit(
        &self,
        model_id: &str,
        capability: Capability,
        payload: JsonValue,
    ) -> Result<String, KernelError> {
        let record = self.record(model_id).await?;
        let entry = self.adapter_for(&record, &capability)?;
        let Some(runner) = entry.job.clone() else {
            return Err(KernelError::RuntimeFailed(format!(
                "{} cannot run {} as a job",
                entry.adapter.id(),
                capability.as_str()
            )));
        };

        let fallback = self.chat_fallback(&capability);
        let configured = merged(
            &record,
            &capability,
            payload,
            fallback.as_deref(),
            None,
            None,
        );
        let seeded_payload = seeded(&configured);

        // The runner runs later, off the front of the queue, so it owns its
        // inputs; `capability`/`seeded_payload` are cloned because the scheduler
        // keeps them on the job record too.
        let run_capability = capability.clone();
        let run_payload = seeded_payload.clone();
        let job: Runner =
            Box::new(move || forward_job_stream(runner.run(&record, run_capability, run_payload)));

        Ok(self
            .scheduler
            .submit(model_id, capability, seeded_payload, job))
    }

    /// Re-run the job that produced `artifact_id` with its original params.
    pub async fn rerun(&self, artifact_id: &str) -> Result<String, KernelError> {
        let artifact = self.artifact(artifact_id).await?;
        self.submit(&artifact.model_id, artifact.capability, artifact.params)
            .await
    }

    /// Re-run the job that produced `artifact_id` with a fresh seed (a variation).
    pub async fn vary(&self, artifact_id: &str) -> Result<String, KernelError> {
        let artifact = self.artifact(artifact_id).await?;
        let params = reseeded(&artifact.params);
        self.submit(&artifact.model_id, artifact.capability, params)
            .await
    }

    async fn artifact(&self, artifact_id: &str) -> Result<Artifact, KernelError> {
        self.artifacts
            .lock()
            .await
            .get(artifact_id)
            .map_err(|error| KernelError::Storage(error.to_string()))?
            .ok_or_else(|| KernelError::ArtifactNotFound(artifact_id.to_owned()))
    }

    /// Whether the model's chat adapter supports tool calls. `false` when the
    /// model is unknown or has no chat runtime.
    pub async fn supports_tools(&self, model_id: &str) -> bool {
        let Ok(record) = self.record(model_id).await else {
            return false;
        };
        self.adapter_for(&record, &Capability::chat())
            .is_ok_and(|entry| entry.adapter.supports_tools(&record))
    }

    /// The request parameter keys the model's adapter honors for `capability`.
    pub async fn honored_params(
        &self,
        model_id: &str,
        capability: Capability,
    ) -> Result<HashSet<String>, KernelError> {
        let record = self.record(model_id).await?;
        let entry = self.adapter_for(&record, &capability)?;
        Ok(entry.adapter.honored_param_keys(&record, &capability))
    }

    /// Every registered model.
    pub async fn shelf(&self) -> Vec<ModelRecord> {
        self.registry
            .lock()
            .await
            .list()
            .into_iter()
            .cloned()
            .collect()
    }

    /// The raw bytes of the artifact stored under `id`, read from disk, or `None`
    /// if no such artifact exists.
    pub async fn artifact_data(&self, id: &str) -> Result<Option<Vec<u8>>, KernelError> {
        // Resolve the on-disk path under the lock, then drop it before the read.
        let path = {
            let mut store = self.artifacts.lock().await;
            store
                .url(id)
                .map_err(|error| KernelError::Storage(error.to_string()))?
        };
        match path {
            Some(path) => tokio::fs::read(&path)
                .await
                .map(Some)
                .map_err(|error| KernelError::Storage(error.to_string())),
            None => Ok(None),
        }
    }

    /// The models currently holding memory, per the governor.
    pub fn resident_models(&self) -> Vec<ResidentEntry> {
        self.governor
            .resident()
            .into_iter()
            .map(|resident| ResidentEntry {
                model_id: Some(resident.model_id),
                name: resident.name,
                footprint_mb: resident.footprint_mb,
            })
            .collect()
    }

    /// A fresh resolution engine over the current adapter set. Built per call
    /// (cheap: `Arc` clones + the builtin profile table), matching the Swift
    /// kernel, which also constructed `ResolutionEngine(adapters:)` on demand.
    fn engine(&self) -> ResolutionEngine {
        let adapters = self
            .adapters
            .iter()
            .map(|entry| Arc::clone(&entry.adapter))
            .collect();
        ResolutionEngine::new(adapters)
    }

    /// Run `scanners` over the machine, reconcile what they find into the registry,
    /// then resolve every record to a runtime. Returns the discovery summary; the
    /// resolved runtimes are written onto the records. This is the discover→serve
    /// path — a model found on disk comes out with a runtime the dispatch layer
    /// can pick. (The Swift `Kernel.discover()` assembled the scanners from the
    /// settings store internally; here the caller passes them, since settings
    /// ownership lives above the runtime crate.)
    ///
    /// Discovery and resolution share one registry-lock hold so the shelf is never
    /// observed half-reconciled. The scanners' blocking filesystem work runs inside
    /// that hold; off-loading it (Swift gated scans behind a separate turnstile) is
    /// deferred — it needs a `Send` bound on `StoreScanner` to cross a task boundary.
    pub async fn discover(
        &self,
        scanners: Vec<Box<dyn StoreScanner>>,
    ) -> Result<DiscoverySummary, KernelError> {
        let mut registry = self.registry.lock().await;
        let summary = DiscoveryService::new(scanners).discover(&mut registry)?;
        self.engine().resolve_all(&mut registry, None)?;
        Ok(summary)
    }

    /// Re-run the resolution auction over the whole registry, writing each record's
    /// winning runtime. Returns the records that changed.
    pub async fn resolve(&self) -> Result<Vec<ModelRecord>, KernelError> {
        let mut registry = self.registry.lock().await;
        Ok(self.engine().resolve_all(&mut registry, None)?)
    }

    /// Explain how every registered model would resolve, without changing
    /// anything — the identification and each adapter's bid, winner-first.
    pub async fn explain(&self) -> Vec<ResolutionExplanation> {
        let registry = self.registry.lock().await;
        self.engine().explain_all(&registry)
    }
}

/// Whether the last chat message carries a non-empty `images` array — the guard
/// that rejects an image payload when the chosen runtime has no vision path.
fn payload_carries_images(payload: &JsonValue) -> bool {
    let Some(messages) = payload
        .as_object()
        .and_then(|fields| fields.get("messages"))
        .and_then(JsonValue::as_array)
    else {
        return false;
    };
    let Some(last) = messages.last().and_then(JsonValue::as_object) else {
        return false;
    };
    last.get("images")
        .and_then(JsonValue::as_array)
        .is_some_and(|images| !images.is_empty())
}

/// The effective context window for `record` under `adapter`: a builtin model's
/// fixed window, otherwise the adapter's (positive) window. `None` leaves the
/// request unbudgeted.
fn effective_window(
    record: &ModelRecord,
    adapter: &dyn RuntimeAdapter,
    requested: Option<i64>,
) -> Option<i64> {
    if record.source.kind == SourceKind::builtin() {
        return Some(BUILTIN_WINDOW);
    }
    adapter
        .effective_context_window(record, requested)
        .filter(|window| *window > 0)
}

/// Assess `configured` against the model's window: reject when the prompt no
/// longer fits, otherwise clamp `max_tokens` to what the window leaves free.
fn clamp_to_window(
    record: &ModelRecord,
    adapter: &dyn RuntimeAdapter,
    mut configured: JsonValue,
) -> Result<JsonValue, KernelError> {
    let requested_context = field_i64(&configured, "context_length");
    let Some(window) = effective_window(record, adapter, requested_context) else {
        return Ok(configured);
    };
    let requested_max = field_i64(&configured, "max_tokens");
    let characters = prompt_characters(&configured);
    match assess(characters, window, requested_max) {
        Verdict::Exceeds { .. } => Err(KernelError::ContextExceeded {
            model: record.name.clone(),
        }),
        Verdict::Fits { clamped_max_tokens } => {
            if let Some(clamped) = clamped_max_tokens
                && (requested_max.is_some() || clamped < IMPLICIT_MAX_TOKENS)
                && let JsonValue::Object(fields) = &mut configured
            {
                fields.insert("max_tokens".to_owned(), JsonValue::Int(clamped));
            }
            Ok(configured)
        }
    }
}

fn field_i64(payload: &JsonValue, key: &str) -> Option<i64> {
    payload
        .as_object()
        .and_then(|fields| fields.get(key))
        .and_then(JsonValue::as_i64)
}

/// Adapt a [`JobStream`] into the scheduler's [`RunnerStream`], mapping the
/// adapter's [`RuntimeError`] onto [`JobError`]. The pump races the adapter's
/// stream against the receiver closing, so when the scheduler cancels (drops the
/// receiver) it drops the underlying stream at once — the adapter observes the
/// cancel without waiting for its next yield.
fn forward_job_stream(mut stream: JobStream) -> RunnerStream {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                item = stream.recv() => item,
                _ = tx.closed() => break,
            };
            let Some(item) = item else { break };
            let mapped = match item {
                Ok(event) => Ok(event),
                Err(RuntimeError::Cancelled) => Err(JobError::Cancelled),
                Err(other) => Err(JobError::Failed(other.to_string())),
            };
            if tx.send(mapped).is_err() {
                break;
            }
        }
    });
    rx
}
