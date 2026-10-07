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
use kernel::manifests::RuntimeManifest;
use kernel::profiles::{BUILTIN_CONTEXT_WINDOW, Verdict, assess, merged, prompt_characters};
use kernel::records::{Capability, JsonValue, ModelRecord, SourceKind};
use kernel::resolution::IdentificationCache;
use kernel::{Registry, RegistryError};
use tokio::sync::{Mutex, mpsc};

use crate::adapters::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError};
use crate::governor::MemoryGovernor;
use crate::jobs::{JobError, JobScheduler, Runner, RunnerStream};
use crate::manifests::StoreLoad;
use crate::resolution::{ResolutionEngine, ResolutionExplanation, StrandedModels};

/// A model window's implicit completion length when the caller sets no
/// `max_tokens`. A clamp at or above this is only written back when the caller
/// asked for a specific `max_tokens`; below it the window itself is the limit,
/// so the clamp is always applied.
const IMPLICIT_MAX_TOKENS: i64 = 4096;

/// The registry's migration level once the GGUF embedders an older shelf
/// stranded have been resolved.
const STRANDED_EMBEDDERS_MIGRATION: u32 = 1;

/// The registry's migration level once the decision GGUFs an older shelf
/// served as chat models or left unresolved have been resolved as judges.
const DECISION_MODELS_MIGRATION: u32 = 2;

/// How many times the first read of a shelf runs one migration when another
/// process keeps writing the store under it.
const MIGRATION_ATTEMPTS: usize = 3;

/// One stranded-model pass of the resolution engine.
type StrandedPass = fn(&ResolutionEngine, &mut Registry) -> Result<StrandedModels, RegistryError>;

/// Run `pass` to bring the store to migration `level`, unless it is there
/// already, and record that it did. The pass changes what it can whatever
/// level the store is at, so a file a lower level could not read never holds
/// up this one's rows. The level is recorded only over the one below it, only
/// when the pass read every file it looked for that is there, so one still
/// being written is looked at again by the next process, and only over the
/// store the pass read: one another process wrote meanwhile is migrated again.
fn migrate(engine: &ResolutionEngine, registry: &mut Registry, level: u32, pass: StrandedPass) {
    for _ in 0..MIGRATION_ATTEMPTS {
        let reached = registry.migration_level();
        if reached >= level {
            return;
        }
        let mut migrated = registry.records();
        let Ok(pass) = pass(engine, registry) else {
            return;
        };
        if pass.unread > 0 || reached + 1 < level {
            return;
        }
        for record in pass.changed {
            migrated.insert(record.id.clone(), record);
        }
        match registry.mark_migrated(level, &migrated) {
            Ok(false) => continue,
            Ok(true) | Err(_) => return,
        }
    }
}

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
    /// The model id. Only governor residents are reported, so it is always
    /// set today; models the Ollama daemon loads on its own are found by the
    /// CLI asking the daemon, not through this list.
    pub model_id: Option<String>,
    /// The model's display name.
    pub name: String,
    /// The footprint in mebibytes.
    pub footprint_mb: i64,
    /// When the idle unload fires, in Unix milliseconds, if a timer is armed.
    pub expires_at_millis: Option<i64>,
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

    /// The id of the runtime this adapter serves.
    pub fn id(&self) -> String {
        self.adapter.id().as_str().to_owned()
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
    shelf_migrated: std::sync::atomic::AtomicBool,
    identification_cache: Arc<IdentificationCache>,
    shelf_snapshot: StdMutex<Option<(u64, Arc<[ModelRecord]>)>>,
    manifest_runtimes: std::sync::OnceLock<StoreLoad>,
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
            shelf_migrated: std::sync::atomic::AtomicBool::new(false),
            identification_cache: Arc::new(IdentificationCache::new()),
            shelf_snapshot: StdMutex::new(None),
            manifest_runtimes: std::sync::OnceLock::new(),
        }
    }

    /// Record the manifest runtimes this kernel's adapters were built from, and
    /// the issues loading them raised. Set once, at boot; a second call is
    /// ignored, since the adapter list it would describe is already fixed.
    pub fn set_manifest_runtimes(&self, runtimes: StoreLoad) {
        let _ = self.manifest_runtimes.set(runtimes);
    }

    /// The manifest runtimes loaded at boot, approved or not.
    pub fn manifest_runtimes(&self) -> &[RuntimeManifest] {
        self.manifest_runtimes
            .get()
            .map_or(&[], |load| load.manifests.as_slice())
    }

    /// What went wrong loading manifest runtimes at boot, one line per entry.
    pub fn runtime_issues(&self) -> &[String] {
        self.manifest_runtimes
            .get()
            .map_or(&[], |load| load.issues.as_slice())
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
    /// store's default system prompt.)
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

    /// The speech voices available for a speak-capable model, from its bundled
    /// `voices/` directory. Empty for a model that bundles none; errors if the
    /// model is unknown.
    pub async fn voices(&self, model_id: &str) -> Result<Vec<String>, KernelError> {
        let record = self.record(model_id).await?;
        Ok(crate::sidecar::speech_voices(&record))
    }

    /// The most tokens one input to `record` may hold, for a model that only
    /// embeds and whose runtime bounds what it reads: llama.cpp caps an
    /// embedder's own window, and a decoder that pools by the last token takes
    /// one token less than that. `None` for any other model, or one whose
    /// runtime sets none.
    pub fn embedding_window(&self, record: &ModelRecord) -> Option<i64> {
        let claims = |capability: Capability| record.capabilities.contains(&capability);
        if !claims(Capability::embed())
            || claims(Capability::chat())
            || claims(Capability::complete())
        {
            return None;
        }
        let entry = self.adapter_for(record, &Capability::embed()).ok()?;
        effective_window(record, entry.adapter.as_ref(), None)
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

    /// Re-read the registry from disk, picking up what another process wrote,
    /// and say whether anything changed.
    ///
    /// A pull runs in a worker of its own and registers what it fetched there,
    /// so a long-lived front end has to be told to look again; nothing else it
    /// does would reload a record it did not write itself.
    pub async fn reload_registry(&self) -> Result<bool, KernelError> {
        let mut registry = self.registry.lock().await;
        Ok(registry.refresh()?)
    }

    /// Every registered model, as a shared snapshot. The snapshot is rebuilt only
    /// when the registry's generation has moved since the last call, so repeated
    /// calls between mutations are a refcount bump rather than a fresh deep clone
    /// of every record — this runs on the path of every chat/embed/generate/image
    /// request via the gateway resolver.
    pub async fn shelf(&self) -> Arc<[ModelRecord]> {
        let mut registry = self.registry.lock().await;
        // Once per process, bring an older shelf up to date without a rescan:
        // refold `tools` for records that predate it (or a fold-rule change)
        // and take back the cross-encoders it registered as embedders. Once
        // per shelf, resolve the GGUF embedders it left unresolved or served
        // as chat models, then the decision GGUFs it did the same to. Both
        // re-read the header of every llama.cpp chat model, and the second
        // also re-identifies every unresolved record, so the store records
        // that each ran. A failed migration serves the shelf as it stands,
        // exactly as before the migration existed.
        if !self
            .shelf_migrated
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let engine = self.engine();
            let _ = engine.refold_tool_capability(&mut registry);
            let _ = engine.reclassify_cross_encoders(&mut registry);
            migrate(
                &engine,
                &mut registry,
                STRANDED_EMBEDDERS_MIGRATION,
                ResolutionEngine::resolve_stranded_embedders,
            );
            migrate(
                &engine,
                &mut registry,
                DECISION_MODELS_MIGRATION,
                ResolutionEngine::resolve_stranded_judges,
            );
        }
        let generation = registry.generation();
        // Built while the registry is still locked, so the generation read above
        // and the snapshot contents can never straddle an intervening mutation.
        match self.shelf_snapshot.lock() {
            Ok(mut cache) => match cache.as_ref() {
                Some((cached_generation, snapshot)) if *cached_generation == generation => {
                    Arc::clone(snapshot)
                }
                _ => {
                    let snapshot: Arc<[ModelRecord]> = registry
                        .list()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .into();
                    *cache = Some((generation, Arc::clone(&snapshot)));
                    snapshot
                }
            },
            // A poisoned cache lock is served by rebuilding fresh rather than
            // propagating the panic — the cache is a pure optimization, never the
            // source of truth.
            Err(_) => registry
                .list()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
                .into(),
        }
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
                expires_at_millis: self.governor.idle_deadline_millis(&resident.model_id),
                model_id: Some(resident.model_id),
                name: resident.name,
                footprint_mb: resident.footprint_mb,
            })
            .collect()
    }

    /// A fresh resolution engine over the current adapter set. Built per call
    /// (cheap: `Arc` clones + the builtin profile table), so the engine is
    /// constructed on demand rather than cached.
    fn engine(&self) -> ResolutionEngine {
        let adapters = self
            .adapters
            .iter()
            .map(|entry| Arc::clone(&entry.adapter))
            .collect();
        ResolutionEngine::new(adapters).with_cache(Arc::clone(&self.identification_cache))
    }

    /// Run `scanners` over the machine, reconcile what they find into the registry,
    /// then resolve every record to a runtime. Returns the discovery summary; the
    /// resolved runtimes are written onto the records. This is the discover→serve
    /// path — a model found on disk comes out with a runtime the dispatch layer
    /// can pick. (The caller passes the scanners, since settings ownership lives
    /// above the runtime crate.)
    ///
    /// Discovery and resolution share one registry-lock hold so the shelf is never
    /// observed half-reconciled. The scanners' blocking filesystem work runs inside
    /// that hold; off-loading it onto a separate turnstile is deferred — it needs a
    /// `Send` bound on `StoreScanner` to cross a task boundary.
    pub async fn discover(
        &self,
        scanners: Vec<Box<dyn StoreScanner>>,
    ) -> Result<DiscoverySummary, KernelError> {
        let mut registry = self.registry.lock().await;
        let mut summary = DiscoveryService::new(scanners).discover(&mut registry)?;
        self.engine().resolve_all(&mut registry, None)?;
        // A manifest that failed to load is a recipe the user wrote and needs
        // told about, in the same place a scan reports everything else.
        summary.issues.extend(self.runtime_issues().iter().cloned());
        Ok(summary)
    }

    /// Re-run the resolution auction over the whole registry, writing each record's
    /// winning runtime. Returns the records that changed.
    pub async fn resolve(&self) -> Result<Vec<ModelRecord>, KernelError> {
        let mut registry = self.registry.lock().await;
        Ok(self.engine().resolve_all(&mut registry, None)?)
    }

    /// Drop the record `id` from the registry, returning it if it was present.
    /// This is the shelf-side of a deletion: once its weights are trashed (or its
    /// Ollama tag deleted), the record is forgotten so it stops appearing on the
    /// shelf. Without this a removed model lingers in `models.json` and a rescan
    /// only re-flags it `Missing` rather than dropping it.
    pub async fn forget(&self, id: &str) -> Result<Option<ModelRecord>, KernelError> {
        let mut registry = self.registry.lock().await;
        Ok(registry.unregister(id)?)
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
        return Some(BUILTIN_CONTEXT_WINDOW);
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kernel::records::{Modality, ModelSource, ModelState, RuntimeId};
    use kernel::resolution::{IdentifiedModel, RuntimeBid};

    use super::*;

    /// Writes the store through a handle of its own, as another process
    /// would, each time the migration plans a record, until it has written
    /// `writes` times.
    struct Interloper {
        id: RuntimeId,
        store: PathBuf,
        writes: usize,
        planned: AtomicUsize,
    }

    impl RuntimeAdapter for Interloper {
        fn id(&self) -> &RuntimeId {
            &self.id
        }

        fn can_serve(&self, _record: &ModelRecord, _capability: &Capability) -> bool {
            false
        }

        fn invoke(
            &self,
            _record: &ModelRecord,
            _capability: Capability,
            _payload: JsonValue,
        ) -> ChunkStream {
            ChunkStream::channel().1
        }

        fn bid(&self, _record: &ModelRecord, _identified: &IdentifiedModel) -> Option<RuntimeBid> {
            let planned = self.planned.fetch_add(1, Ordering::Relaxed);
            if planned < self.writes {
                let name = format!("written-meanwhile-{planned}");
                let mut other = Registry::open(&self.store).unwrap();
                other.register(chat_record(&self.store, &name)).unwrap();
            }
            None
        }
    }

    /// A ready chat model, which neither migration selects.
    fn chat_record(store: &Path, name: &str) -> ModelRecord {
        let path = store.join(name);
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::file(), &path.to_string_lossy()),
        );
        record.state = ModelState::Ready;
        record
    }

    /// Run migration `level` by `pass` over a store at the level below it
    /// holding one stranded embedder whose file is gone, with another writer
    /// that writes `writes` times while it runs. Returns how many passes
    /// planned that row and the level marked.
    fn migrate_while_written(level: u32, pass: StrandedPass, writes: usize) -> (usize, u32) {
        let store = std::env::temp_dir().join(format!(
            "hedos-facade-migration-{}-{level}-{writes}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&store);
        let mut registry = Registry::open(&store).unwrap();
        let mut stranded = ModelRecord::new(
            "stranded.gguf",
            Modality::embedding(),
            vec![Capability::embed()],
            ModelSource::new(
                SourceKind::file(),
                &store.join("stranded.gguf").to_string_lossy(),
            ),
        );
        stranded.state = ModelState::Unresolved;
        registry.register(stranded).unwrap();
        if level > 1 {
            let records = registry.records();
            assert!(registry.mark_migrated(level - 1, &records).unwrap());
        }
        let interloper = Arc::new(Interloper {
            id: RuntimeId::from("interloper"),
            store: store.clone(),
            writes,
            planned: AtomicUsize::new(0),
        });
        let engine = ResolutionEngine::new(vec![interloper.clone()]);
        migrate(&engine, &mut registry, level, pass);
        let reached = Registry::open(&store).unwrap().migration_level();
        let _ = std::fs::remove_dir_all(&store);
        (interloper.planned.load(Ordering::Relaxed), reached)
    }

    /// Each migration level with the pass that reaches it.
    const LEVELS: [(u32, StrandedPass); 2] = [
        (
            STRANDED_EMBEDDERS_MIGRATION,
            ResolutionEngine::resolve_stranded_embedders,
        ),
        (
            DECISION_MODELS_MIGRATION,
            ResolutionEngine::resolve_stranded_judges,
        ),
    ];

    #[test]
    fn a_store_written_during_the_migration_is_migrated_again_then_marked() {
        for (level, pass) in LEVELS {
            assert_eq!(migrate_while_written(level, pass, 0), (1, level));
            assert_eq!(migrate_while_written(level, pass, 1), (2, level));
        }
    }

    #[test]
    fn a_store_written_on_every_pass_is_left_unmarked_after_the_last_attempt() {
        for (level, pass) in LEVELS {
            let (planned, reached) = migrate_while_written(level, pass, usize::MAX);
            assert_eq!(planned, MIGRATION_ATTEMPTS, "{level}");
            assert_eq!(reached, level - 1, "{level}");
        }
    }
}
