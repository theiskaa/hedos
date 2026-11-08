//! A configurable [`GatewayPort`] double for exercising the resolver gate and the
//! handlers without a running kernel.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gateway::admission::{GatewayAdmissionState, GatewayWorkKind};
use gateway::auto_route::AutoRoute;
use gateway::port::{GatewayPort, GatewayResident, PortFuture};
use kernel::capabilities::CapabilityChunk;
use kernel::jobs::{Job, JobEvent};
use kernel::profiles::{Verdict, assess, prompt_characters};
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, ModelState, SourceKind,
};
use kernel::routing::RouteRules;
use runtime::adapters::{ChunkStream, RuntimeError};
use runtime::facade::KernelError;
use tokio::sync::mpsc;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory that removes itself when dropped. Built without
/// the `tempfile` crate to honor the project's minimal-dependency policy.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a fresh, process-unique temporary directory.
    pub fn new() -> Self {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("hedos-gateway-test-{pid}-{nanos}-{unique}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path to `name` inside this directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A port whose behavior is fixed at construction.
pub struct MockPort {
    pub shelf: Vec<ModelRecord>,
    pub chunks: Vec<CapabilityChunk>,
    pub honored: HashSet<String>,
    /// Parameter keys honored for a particular record id, in place of
    /// `honored`.
    pub honored_by: HashMap<String, HashSet<String>>,
    pub admission: GatewayAdmissionState,
    /// Voices returned by `voices`, first one used as the speech default.
    pub voices: Vec<String>,
    /// Events replayed, in order, from `job_events`.
    pub job_events: Vec<JobEvent>,
    /// Artifact bytes keyed by id, served by `artifact_data`.
    pub artifacts: HashMap<String, Vec<u8>>,
    /// Models reported as held in memory by `resident`.
    pub resident: Vec<GatewayResident>,
    /// Every `invoke` this port received, in order: the capability, the
    /// payload, and the record id.
    pub invoked: Mutex<Vec<(Capability, JsonValue, String)>>,
    /// When set, each invoke's stream is left open after `chunks`, its sender
    /// kept here, so a test can see whether the reader let it go.
    pub open_streams: Option<Mutex<Vec<StreamSender>>>,
    /// When set, each invoke's stream ends with this error after `chunks`.
    pub stream_error: Option<RuntimeError>,
    /// Every `submit` this port received, in order: the capability and payload.
    pub submitted: Mutex<Vec<(Capability, JsonValue)>>,
    /// Every job id `cancel` was called with.
    pub cancelled: Mutex<Vec<String>>,
    /// When set, a job's event stream stays open after `job_events`, its
    /// sender kept here, as a job still running does.
    pub open_jobs: Option<Mutex<Vec<mpsc::UnboundedSender<JobEvent>>>>,
    /// The `auto` routing this port serves, if any.
    /// Shared, so two ports can serve one routing's memory.
    pub auto_route: Option<Arc<AutoRoute>>,
    /// The context window each model id is served at; models not listed fall
    /// back to their record's declared window.
    pub windows: HashMap<String, i64>,
}

/// The sending half of a runtime's chunk stream.
pub type StreamSender = mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>;

impl Default for MockPort {
    fn default() -> Self {
        Self {
            shelf: Vec::new(),
            chunks: Vec::new(),
            honored: HashSet::new(),
            honored_by: HashMap::new(),
            admission: GatewayAdmissionState::Ready,
            voices: Vec::new(),
            job_events: Vec::new(),
            artifacts: HashMap::new(),
            resident: Vec::new(),
            invoked: Mutex::new(Vec::new()),
            open_streams: None,
            stream_error: None,
            submitted: Mutex::new(Vec::new()),
            cancelled: Mutex::new(Vec::new()),
            open_jobs: None,
            auto_route: None,
            windows: HashMap::new(),
        }
    }
}

impl MockPort {
    /// A port serving a single ready model with the given id/name.
    pub fn with_ready_model(name: &str) -> (Self, String) {
        let record = ready_model(name);
        let id = record.id.clone();
        (
            Self {
                shelf: vec![record],
                ..Self::default()
            },
            id,
        )
    }

    /// A port serving a single ready model that offers `capability`.
    pub fn with_capable_model(name: &str, capability: Capability) -> (Self, String) {
        let record = capable_model(name, capability);
        let id = record.id.clone();
        (
            Self {
                shelf: vec![record],
                ..Self::default()
            },
            id,
        )
    }
}

/// A port with `tiny` and `big` on the shelf, each serving chat, completion
/// and tools, and `auto` routing between them.
pub fn routed_port() -> MockPort {
    let model = |name: &str| {
        let mut record = capable_model(name, Capability::chat());
        record.capabilities = vec![
            Capability::chat(),
            Capability::complete(),
            Capability::tools(),
        ];
        record
    };
    MockPort {
        shelf: vec![model("tiny"), model("big")],
        chunks: vec![
            CapabilityChunk::Text("ok".to_owned()),
            CapabilityChunk::Done(None),
        ],
        auto_route: Some(Arc::new(AutoRoute::new(
            "auto",
            "tiny",
            "big",
            RouteRules::default(),
            Duration::from_secs(1800),
        ))),
        ..MockPort::default()
    }
}

/// The names of the models `port` was invoked on, in order.
pub fn invoked_names(port: &MockPort) -> Vec<String> {
    port.invoked
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, id)| {
            port.shelf
                .iter()
                .find(|record| &record.id == id)
                .map(|record| record.name.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// A ready text chat model registered from an ollama tag.
pub fn ready_model(name: &str) -> ModelRecord {
    capable_model(name, Capability::chat())
}

/// A ready model offering a single `capability`, registered from an ollama tag.
pub fn capable_model(name: &str, capability: Capability) -> ModelRecord {
    let mut record = ModelRecord::new(
        name,
        Modality::text(),
        vec![capability],
        ModelSource::new(SourceKind::ollama(), name),
    );
    record.state = ModelState::Ready;
    record
}

impl GatewayPort for MockPort {
    fn shelf(&self) -> PortFuture<'_, Arc<[ModelRecord]>> {
        let shelf: Arc<[ModelRecord]> = self.shelf.clone().into();
        Box::pin(async move { shelf })
    }

    fn resident(&self) -> PortFuture<'_, Vec<GatewayResident>> {
        let resident = self.resident.clone();
        Box::pin(async move { resident })
    }

    fn invoke<'a>(
        &'a self,
        model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<ChunkStream, KernelError>> {
        self.invoked
            .lock()
            .unwrap()
            .push((capability, payload, model_id.to_owned()));
        let chunks = self.chunks.clone();
        Box::pin(async move {
            let (tx, stream) = ChunkStream::channel();
            for chunk in chunks {
                let _ = tx.send(Ok(chunk));
            }
            if let Some(error) = &self.stream_error {
                let _ = tx.send(Err(error.clone()));
            }
            if let Some(open) = &self.open_streams {
                open.lock().unwrap().push(tx);
            }
            Ok(stream)
        })
    }

    fn submit<'a>(
        &'a self,
        _model_id: &'a str,
        capability: Capability,
        payload: JsonValue,
    ) -> PortFuture<'a, Result<String, KernelError>> {
        self.submitted.lock().unwrap().push((capability, payload));
        Box::pin(async { Ok("job-1".to_owned()) })
    }

    fn job<'a>(&'a self, _id: &'a str) -> PortFuture<'a, Option<Job>> {
        Box::pin(async { None })
    }

    fn job_events<'a>(&'a self, _id: &'a str) -> PortFuture<'a, mpsc::UnboundedReceiver<JobEvent>> {
        let events = self.job_events.clone();
        Box::pin(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            for event in events {
                let _ = tx.send(event);
            }
            if let Some(open) = &self.open_jobs {
                open.lock().unwrap().push(tx);
            }
            rx
        })
    }

    fn cancel<'a>(&'a self, job_id: &'a str) -> PortFuture<'a, ()> {
        self.cancelled.lock().unwrap().push(job_id.to_owned());
        Box::pin(async {})
    }

    fn abandon(&self, job_id: &str) {
        self.cancelled.lock().unwrap().push(job_id.to_owned());
    }

    fn voices<'a>(
        &'a self,
        _model_id: &'a str,
    ) -> PortFuture<'a, Result<Vec<String>, KernelError>> {
        let voices = self.voices.clone();
        Box::pin(async move { Ok(voices) })
    }

    fn honored_params<'a>(
        &'a self,
        model_id: &'a str,
        _capability: Capability,
    ) -> PortFuture<'a, Result<HashSet<String>, KernelError>> {
        let honored = self
            .honored_by
            .get(model_id)
            .unwrap_or(&self.honored)
            .clone();
        Box::pin(async move { Ok(honored) })
    }

    fn artifact_data<'a>(
        &'a self,
        id: &'a str,
    ) -> PortFuture<'a, Result<Option<Vec<u8>>, KernelError>> {
        let data = self.artifacts.get(id).cloned();
        Box::pin(async move { Ok(data) })
    }

    fn prompt_fit(
        &self,
        record: &ModelRecord,
        _capability: &Capability,
        payload: &JsonValue,
        uncounted: i64,
    ) -> Verdict {
        match self
            .windows
            .get(&record.id)
            .copied()
            .or(record.context_length)
        {
            Some(window) => assess(prompt_characters(payload) + uncounted, window, None),
            None => Verdict::Fits {
                clamped_max_tokens: None,
            },
        }
    }

    fn auto_route(&self) -> Option<&AutoRoute> {
        self.auto_route.as_deref()
    }

    fn admission_state<'a>(
        &'a self,
        _model_id: &'a str,
        _footprint_mb: Option<i64>,
        _kind: GatewayWorkKind,
    ) -> PortFuture<'a, GatewayAdmissionState> {
        let admission = self.admission;
        Box::pin(async move { admission })
    }
}
