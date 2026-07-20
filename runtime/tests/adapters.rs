//! Tests for the runtime adapter interface: dynamic dispatch through
//! `RuntimeAdapter`/`JobRunning`, the default methods, the stream helpers, and
//! the `SidecarError` → `RuntimeError` conversion.

use std::collections::HashSet;

use kernel::capabilities::CapabilityChunk;
use kernel::jobs::JobRuntimeEvent;
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{
    ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream,
};
use runtime::sidecar::SidecarError;

fn record() -> ModelRecord {
    ModelRecord::new(
        "Test",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/tmp/model"),
    )
}

struct EchoAdapter {
    id: RuntimeId,
}

impl RuntimeAdapter for EchoAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, _record: &ModelRecord, capability: &Capability) -> bool {
        *capability == Capability::chat()
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let prompt = payload
            .as_object()
            .and_then(|fields| fields.get("prompt"))
            .and_then(JsonValue::as_str)
            .unwrap_or("")
            .to_owned();
        tokio::spawn(async move {
            let _ = tx.send(Ok(CapabilityChunk::Text(prompt)));
            let _ = tx.send(Ok(CapabilityChunk::Done(None)));
        });
        stream
    }
}

struct FakeImageRunner;

impl JobRunning for FakeImageRunner {
    fn run(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        _payload: JsonValue,
    ) -> JobStream {
        let (tx, stream) = JobStream::channel();
        tokio::spawn(async move {
            let _ = tx.send(Ok(JobRuntimeEvent::Started));
            let _ = tx.send(Ok(JobRuntimeEvent::Artifacts(vec!["art-1".to_owned()])));
        });
        stream
    }
}

#[tokio::test]
async fn an_adapter_serves_through_dynamic_dispatch() {
    let adapter: Box<dyn RuntimeAdapter> = Box::new(EchoAdapter {
        id: RuntimeId::from("fake:echo"),
    });
    let record = record();

    assert_eq!(adapter.id().as_str(), "fake:echo");
    assert!(adapter.can_serve(&record, &Capability::chat()));
    assert!(!adapter.can_serve(&record, &Capability::image()));

    let payload = JsonValue::Object(
        [("prompt".to_owned(), JsonValue::String("hi".to_owned()))]
            .into_iter()
            .collect(),
    );
    let mut stream = adapter.invoke(&record, Capability::chat(), payload);
    let mut chunks = Vec::new();
    while let Some(item) = stream.recv().await {
        chunks.push(item.expect("no error"));
    }
    assert_eq!(
        chunks,
        vec![
            CapabilityChunk::Text("hi".to_owned()),
            CapabilityChunk::Done(None)
        ]
    );
}

#[tokio::test]
async fn adapter_defaults_are_conservative() {
    let adapter = EchoAdapter {
        id: RuntimeId::from("fake:echo"),
    };
    let record = record();
    assert!(!adapter.wires_tools());
    assert_eq!(adapter.effective_context_window(&record, Some(4096)), None);
    assert_eq!(
        adapter.honored_param_keys(&record, &Capability::chat()),
        HashSet::new()
    );
}

#[tokio::test]
async fn a_job_runner_streams_events() {
    let runner: Box<dyn JobRunning> = Box::new(FakeImageRunner);
    let record = record();
    let mut stream = runner.run(&record, Capability::image(), JsonValue::Null);
    let mut events = Vec::new();
    while let Some(item) = stream.recv().await {
        events.push(item.expect("no error"));
    }
    assert_eq!(
        events,
        vec![
            JobRuntimeEvent::Started,
            JobRuntimeEvent::Artifacts(vec!["art-1".to_owned()])
        ]
    );
}

#[tokio::test]
async fn a_failed_stream_yields_one_error() {
    let mut stream: RuntimeStream<CapabilityChunk> =
        RuntimeStream::failed(RuntimeError::WrongExecutionMode);
    let first = stream.recv().await.expect("one item");
    assert!(matches!(first, Err(RuntimeError::WrongExecutionMode)));
    assert!(stream.recv().await.is_none(), "then the stream ends");
}

#[test]
fn sidecar_errors_convert_to_runtime_errors() {
    assert!(matches!(
        RuntimeError::from(SidecarError::Cancelled),
        RuntimeError::Cancelled
    ));
    assert!(matches!(
        RuntimeError::from(SidecarError::RuntimeFailed("boom".to_owned())),
        RuntimeError::Failed(m) if m == "boom"
    ));
    assert!(matches!(
        RuntimeError::from(SidecarError::SidecarDied {
            runtime_id: "r".to_owned(),
            detail: "died".to_owned()
        }),
        RuntimeError::Failed(m) if m == "sidecar r died"
    ));
}
