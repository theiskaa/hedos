//! Tests for the `Kernel` facade: request resolution, the shared prompt/param/
//! context policy on the streaming path, the governor-backed job path, and the
//! provenance artifact writer that lands a finished job on disk.

mod support;

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use kernel::Registry;
use kernel::artifacts::ArtifactStore;
use kernel::capabilities::CapabilityChunk;
use kernel::jobs::{JobHistoryStore, JobRuntimeEvent, JobState};
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{ChunkStream, JobRunning, JobStream, RuntimeAdapter};
use runtime::facade::{Kernel, KernelError, RegisteredAdapter};
use runtime::governor::{GovernorConfig, MemoryGovernor};
use support::TempDir;

/// A configurable fake backend that records what it was asked to serve.
struct Fake {
    id: RuntimeId,
    serves: Vec<Capability>,
    window: Option<i64>,
    tools: bool,
    job_result: Option<(Vec<u8>, String)>,
    last_invoke: Arc<StdMutex<Option<JsonValue>>>,
    last_job: Arc<StdMutex<Option<JsonValue>>>,
}

impl Fake {
    fn new(serves: Vec<Capability>) -> Self {
        Self {
            id: RuntimeId::ollama(),
            serves,
            window: None,
            tools: false,
            job_result: None,
            last_invoke: Arc::new(StdMutex::new(None)),
            last_job: Arc::new(StdMutex::new(None)),
        }
    }

    fn window(mut self, window: i64) -> Self {
        self.window = Some(window);
        self
    }

    fn tools(mut self, tools: bool) -> Self {
        self.tools = tools;
        self
    }

    fn job_result(mut self, data: &[u8], ext: &str) -> Self {
        self.job_result = Some((data.to_vec(), ext.to_owned()));
        self
    }
}

impl RuntimeAdapter for Fake {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, _record: &ModelRecord, capability: &Capability) -> bool {
        self.serves.contains(capability)
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        *self.last_invoke.lock().expect("lock") = Some(payload);
        let (tx, stream) = ChunkStream::channel();
        let _ = tx.send(Ok(CapabilityChunk::Text("ok".to_owned())));
        let _ = tx.send(Ok(CapabilityChunk::Done(None)));
        stream
    }

    fn effective_context_window(
        &self,
        _record: &ModelRecord,
        _requested: Option<i64>,
    ) -> Option<i64> {
        self.window
    }

    fn supports_tools(&self, _record: &ModelRecord) -> bool {
        self.tools
    }

    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        _capability: &Capability,
    ) -> HashSet<String> {
        ["temperature", "max_tokens"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

impl JobRunning for Fake {
    fn run(&self, _record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        *self.last_job.lock().expect("lock") = Some(payload);
        let (tx, stream) = JobStream::channel();
        let _ = tx.send(Ok(JobRuntimeEvent::Started));
        if let Some((data, ext)) = self.job_result.clone() {
            let _ = tx.send(Ok(JobRuntimeEvent::Result {
                data,
                file_extension: ext,
            }));
        }
        stream
    }
}

fn record(id: &str) -> ModelRecord {
    let mut record = ModelRecord::new(
        id,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), id),
    );
    record.runtime.id = Some(RuntimeId::ollama());
    record.id = id.to_owned();
    record
}

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn text(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}

fn chat_payload(content: &str) -> JsonValue {
    let message = object([("role", text("user")), ("content", text(content))]);
    object([("messages", JsonValue::Array(vec![message]))])
}

fn kernel(dir: &TempDir, record: Option<ModelRecord>, adapters: Vec<RegisteredAdapter>) -> Kernel {
    let mut registry = Registry::open(dir.path()).expect("registry");
    if let Some(record) = record {
        registry.register(record).expect("register");
    }
    let artifacts = ArtifactStore::new(dir.path());
    let governor = Arc::new(MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)));
    let history = JobHistoryStore::with_default_limit(dir.path());
    Kernel::new(registry, artifacts, governor, history, adapters)
}

async fn collect_text(mut stream: ChunkStream) -> String {
    let mut text = String::new();
    while let Some(item) = stream.recv().await {
        if let Ok(CapabilityChunk::Text(part)) = item {
            text.push_str(&part);
        }
    }
    text
}

async fn await_job(kernel: &Kernel, id: &str) -> kernel::jobs::Job {
    for _ in 0..400 {
        if let Some(job) = kernel.scheduler().job(id)
            && matches!(
                job.state,
                JobState::Done | JobState::Failed | JobState::Cancelled
            )
        {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("job did not finish in time");
}

#[tokio::test]
async fn invoke_streams_after_resolving_the_adapter() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let seen = Arc::clone(&fake.last_invoke);
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let stream = kernel
        .invoke("m", Capability::chat(), chat_payload("hi"))
        .await
        .expect("invoke");
    assert_eq!(collect_text(stream).await, "ok");
    assert!(seen.lock().unwrap().is_some(), "adapter was invoked");
}

#[tokio::test]
async fn invoke_injects_the_records_system_prompt() {
    let dir = TempDir::new();
    let mut record = record("m");
    record.system_prompt = Some("be brief".to_owned());
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let seen = Arc::clone(&fake.last_invoke);
    let kernel = kernel(&dir, Some(record), vec![RegisteredAdapter::streaming(fake)]);

    let _ = kernel
        .invoke("m", Capability::chat(), chat_payload("hi"))
        .await
        .expect("invoke");

    let payload = seen.lock().unwrap().clone().expect("payload");
    let messages = payload
        .as_object()
        .and_then(|o| o.get("messages"))
        .and_then(JsonValue::as_array)
        .expect("messages");
    let first = messages[0].as_object().expect("system turn");
    assert_eq!(first.get("role"), Some(&text("system")));
    assert_eq!(first.get("content"), Some(&text("be brief")));
}

#[tokio::test]
async fn invoke_with_applies_the_session_override_and_suffix() {
    let dir = TempDir::new();
    let mut record = record("m");
    record.system_prompt = Some("record prompt".to_owned());
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let seen = Arc::clone(&fake.last_invoke);
    let kernel = kernel(&dir, Some(record), vec![RegisteredAdapter::streaming(fake)]);

    let _ = kernel
        .invoke_with(
            "m",
            Capability::chat(),
            chat_payload("hi"),
            Some("session override"),
            Some("appended tools block"),
        )
        .await
        .expect("invoke_with");

    let payload = seen.lock().unwrap().clone().expect("payload");
    let messages = payload
        .as_object()
        .and_then(|o| o.get("messages"))
        .and_then(JsonValue::as_array)
        .expect("messages");
    let system = messages[0].as_object().expect("system turn");
    // The session override wins over the record prompt, and the suffix is
    // appended to the same system turn.
    let content = match system.get("content") {
        Some(JsonValue::String(text)) => text.clone(),
        _ => panic!("system content"),
    };
    assert!(content.contains("session override"), "override: {content}");
    assert!(
        content.contains("appended tools block"),
        "suffix: {content}"
    );
    assert!(
        !content.contains("record prompt"),
        "override replaces record"
    );
}

#[tokio::test]
async fn invoke_injects_the_settings_fallback_prompt() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let seen = Arc::clone(&fake.last_invoke);
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );
    kernel.set_default_system_prompt(Some("global default".to_owned()));

    let _ = kernel
        .invoke("m", Capability::chat(), chat_payload("hi"))
        .await
        .expect("invoke");

    let payload = seen.lock().unwrap().clone().expect("payload");
    let messages = payload
        .as_object()
        .and_then(|o| o.get("messages"))
        .and_then(JsonValue::as_array)
        .expect("messages");
    let first = messages[0].as_object().expect("system turn");
    assert_eq!(first.get("content"), Some(&text("global default")));
}

#[tokio::test]
async fn complete_requests_are_budget_clamped() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::complete()]).window(300));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let long = "x".repeat(400);
    let payload = object([("prompt", text(&long))]);
    let error = kernel
        .invoke("m", Capability::complete(), payload)
        .await
        .err()
        .expect("exceeds window");
    assert!(matches!(error, KernelError::ContextExceeded { .. }));
}

#[tokio::test]
async fn honored_params_reports_error_paths() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let unknown = kernel
        .honored_params("ghost", Capability::chat())
        .await
        .expect_err("unknown model");
    assert!(matches!(unknown, KernelError::ModelNotFound(_)));

    let unsupported = kernel
        .honored_params("m", Capability::embed())
        .await
        .expect_err("no embed adapter");
    assert!(matches!(
        unsupported,
        KernelError::CapabilityUnsupported { .. }
    ));
}

#[tokio::test]
async fn artifact_provenance_uses_name_and_id_separately() {
    let dir = TempDir::new();
    let mut record = record("voice-id");
    record.name = "Friendly Voice".to_owned();
    let fake = Arc::new(Fake::new(vec![Capability::speak()]).job_result(b"WAV", "wav"));
    let kernel = kernel(
        &dir,
        Some(record),
        vec![RegisteredAdapter::with_jobs(
            Arc::clone(&fake) as Arc<dyn RuntimeAdapter>,
            fake as Arc<dyn JobRunning>,
        )],
    );

    let id = kernel
        .submit("voice-id", Capability::speak(), object([]))
        .await
        .expect("submit");
    let job = await_job(&kernel, &id).await;
    let artifact_id = job.result.first().cloned().expect("artifact id");

    let mut store = ArtifactStore::new(dir.path());
    let _ = store.list();
    let artifact = store.get(&artifact_id).expect("read").expect("artifact");
    assert_eq!(artifact.model, "Friendly Voice");
    assert_eq!(artifact.model_id, "voice-id");
}

#[tokio::test]
async fn invoke_reports_an_unknown_model() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None, vec![]);
    let error = kernel
        .invoke("ghost", Capability::chat(), chat_payload("hi"))
        .await
        .err()
        .expect("unknown model");
    assert!(matches!(error, KernelError::ModelNotFound(id) if id == "ghost"));
}

#[tokio::test]
async fn invoke_reports_an_unsupported_capability() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::embed()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );
    let error = kernel
        .invoke("m", Capability::chat(), chat_payload("hi"))
        .await
        .err()
        .expect("no chat adapter");
    assert!(matches!(error, KernelError::CapabilityUnsupported { .. }));
}

#[tokio::test]
async fn invoke_rejects_images_without_a_vision_path() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let message = object([
        ("role", text("user")),
        ("content", text("look")),
        ("images", JsonValue::Array(vec![text("base64")])),
    ]);
    let payload = object([("messages", JsonValue::Array(vec![message]))]);
    let error = kernel
        .invoke("m", Capability::chat(), payload)
        .await
        .err()
        .expect("no vision path");
    assert!(matches!(error, KernelError::PayloadInvalid(_)));
}

#[tokio::test]
async fn invoke_passes_images_when_the_adapter_sees() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat(), Capability::see()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let message = object([
        ("role", text("user")),
        ("content", text("look")),
        ("images", JsonValue::Array(vec![text("base64")])),
    ]);
    let payload = object([("messages", JsonValue::Array(vec![message]))]);
    assert!(
        kernel
            .invoke("m", Capability::chat(), payload)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn invoke_rejects_a_prompt_that_exceeds_the_window() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]).window(300));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    let long = "x".repeat(400);
    let error = kernel
        .invoke("m", Capability::chat(), chat_payload(&long))
        .await
        .err()
        .expect("exceeds window");
    assert!(matches!(error, KernelError::ContextExceeded { .. }));
}

#[tokio::test]
async fn invoke_clamps_max_tokens_to_the_window() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]).window(1000));
    let seen = Arc::clone(&fake.last_invoke);
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );

    // "hi" is 2 chars → ~1 token; the window leaves 999 for the completion.
    let _ = kernel
        .invoke("m", Capability::chat(), chat_payload("hi"))
        .await
        .expect("invoke");
    let payload = seen.lock().unwrap().clone().expect("payload");
    let clamped = payload
        .as_object()
        .and_then(|o| o.get("max_tokens"))
        .and_then(JsonValue::as_i64);
    assert_eq!(clamped, Some(999));
}

#[tokio::test]
async fn submit_runs_a_job_and_writes_a_provenanced_artifact() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::speak()]).job_result(b"WAVDATA", "wav"));
    let kernel = kernel(
        &dir,
        Some(record("voice")),
        vec![RegisteredAdapter::with_jobs(
            Arc::clone(&fake) as Arc<dyn RuntimeAdapter>,
            fake as Arc<dyn JobRunning>,
        )],
    );

    let id = kernel
        .submit("voice", Capability::speak(), object([]))
        .await
        .expect("submit");
    let job = await_job(&kernel, &id).await;
    assert_eq!(job.state, JobState::Done);
    let artifact_id = job.result.first().cloned().expect("artifact id");

    let mut store = ArtifactStore::new(dir.path());
    let _ = store.list();
    let artifact = store
        .get(&artifact_id)
        .expect("read")
        .expect("artifact exists");
    assert_eq!(artifact.model, "voice");
    assert_eq!(artifact.runtime, "ollama");
    assert_eq!(artifact.capability, Capability::speak());

    // The governor admitted the job, so the model is now accounted resident.
    assert!(
        kernel
            .resident_models()
            .iter()
            .any(|entry| entry.model_id.as_deref() == Some("voice")),
        "model resident after admission"
    );
}

#[tokio::test]
async fn submit_rejects_an_adapter_that_runs_no_jobs() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::speak()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );
    let error = kernel
        .submit("m", Capability::speak(), object([]))
        .await
        .expect_err("no job runner");
    assert!(matches!(error, KernelError::RuntimeFailed(_)));
}

#[tokio::test]
async fn rerun_and_vary_resubmit_a_stored_artifact() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::speak()]).job_result(b"WAV", "wav"));
    let kernel = kernel(
        &dir,
        Some(record("voice")),
        vec![RegisteredAdapter::with_jobs(
            Arc::clone(&fake) as Arc<dyn RuntimeAdapter>,
            fake as Arc<dyn JobRunning>,
        )],
    );

    let first = kernel
        .submit("voice", Capability::speak(), chat_payload("seed"))
        .await
        .expect("submit");
    let job = await_job(&kernel, &first).await;
    let artifact_id = job.result.first().cloned().expect("artifact id");

    let rerun = kernel.rerun(&artifact_id).await.expect("rerun");
    assert_ne!(rerun, first, "rerun mints a new job");
    let _ = await_job(&kernel, &rerun).await;

    let vary = kernel.vary(&artifact_id).await.expect("vary");
    let _ = await_job(&kernel, &vary).await;
    assert_ne!(vary, rerun);
}

#[tokio::test]
async fn rerun_reports_a_missing_artifact() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, None, vec![]);
    let error = kernel.rerun("nope").await.expect_err("missing");
    assert!(matches!(error, KernelError::ArtifactNotFound(id) if id == "nope"));
}

#[tokio::test]
async fn supports_tools_reflects_the_adapter() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]).tools(true));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );
    assert!(kernel.supports_tools("m").await);
    assert!(!kernel.supports_tools("ghost").await);
}

#[tokio::test]
async fn honored_params_come_from_the_adapter() {
    let dir = TempDir::new();
    let fake = Arc::new(Fake::new(vec![Capability::chat()]));
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(fake)],
    );
    let params = kernel
        .honored_params("m", Capability::chat())
        .await
        .expect("params");
    assert!(params.contains("temperature"));
    assert!(params.contains("max_tokens"));
}

#[tokio::test]
async fn shelf_lists_registered_models() {
    let dir = TempDir::new();
    let kernel = kernel(
        &dir,
        Some(record("m")),
        vec![RegisteredAdapter::streaming(Arc::new(Fake::new(vec![
            Capability::chat(),
        ])))],
    );
    let shelf = kernel.shelf().await;
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].id, "m");
}
