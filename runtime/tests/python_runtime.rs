//! End-to-end test of the governed streaming driver: a fake descriptor drives a
//! real fake sidecar through the full stack (governor admit/gate/residency +
//! sidecar supervisor + the governed load helpers).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue, Modality, ModelRecord, ModelSource, SourceKind};
use runtime::governor::{GovernorConfig, MemoryGovernor};
use runtime::python_runtime::{Descriptor, PythonSidecarRuntime};
use runtime::sidecar::{SidecarSpec, SidecarSupervisor};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn record() -> ModelRecord {
    let mut record = ModelRecord::new(
        "Test Model",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/tmp/test-model"),
    );
    record.footprint_mb = Some(2048);
    record
}

fn descriptor() -> Descriptor {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_sidecar.py").to_owned();
    let runtime_id = format!("py-{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let spec_id = runtime_id.clone();
    Descriptor {
        runtime_id,
        preparing_status: "Preparing".to_owned(),
        starting_status: "Starting".to_owned(),
        warm_window: None,
        prepare_environment: Arc::new(|sink| {
            sink("Preparing the environment");
            Box::pin(async { Ok(None) })
        }),
        make_spec: Arc::new(move |_record: &ModelRecord, _env: Option<&Path>| {
            let mut spec = SidecarSpec::new(
                spec_id.clone(),
                PathBuf::from("/usr/bin/env"),
                vec!["python3".to_owned(), script.clone(), "normal".to_owned()],
            );
            spec.ready_timeout = Duration::from_secs(15);
            spec.cooperative_cancel = true;
            Ok(spec)
        }),
    }
}

fn chat_payload(content: &str) -> JsonValue {
    let message = JsonValue::Object(BTreeMap::from([
        ("role".to_owned(), JsonValue::String("user".to_owned())),
        ("content".to_owned(), JsonValue::String(content.to_owned())),
    ]));
    JsonValue::Object(BTreeMap::from([(
        "messages".to_owned(),
        JsonValue::Array(vec![message]),
    )]))
}

#[tokio::test]
async fn drives_a_governed_chat_stream_to_completion() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let supervisor = SidecarSupervisor::new();
    let record = record();
    let descriptor = descriptor();
    let runtime_id = descriptor.runtime_id.clone();
    let runtime = PythonSidecarRuntime::new(descriptor, governor.clone(), supervisor.clone());

    let mut stream = runtime.stream(&record, Capability::chat(), chat_payload("abcdef"));
    let mut statuses = Vec::new();
    let mut text = String::new();
    let mut done = false;
    while let Some(item) = stream.recv().await {
        match item.expect("no error") {
            CapabilityChunk::Status(message) => statuses.push(message),
            CapabilityChunk::Text(delta) => text.push_str(&delta),
            CapabilityChunk::Done(_) => done = true,
            _ => {}
        }
    }

    assert_eq!(text, "abcdef!");
    assert!(done, "the stream reached done");
    assert!(
        statuses.iter().any(|s| s == "Preparing"),
        "preparing status: {statuses:?}"
    );
    assert!(
        statuses.iter().any(|s| s == "Starting"),
        "starting status: {statuses:?}"
    );
    assert!(
        governor.is_resident(&record.id),
        "the model is a governed resident"
    );
    assert_eq!(
        governor.leases().count(&record.id),
        0,
        "the generation lease is balanced"
    );

    supervisor.shutdown(&runtime_id).await;
}

fn slow_prep_descriptor() -> Descriptor {
    let mut descriptor = descriptor();
    descriptor.prepare_environment = Arc::new(|sink| {
        sink("Preparing the environment");
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok(None)
        })
    });
    descriptor
}

#[tokio::test]
async fn dropping_during_env_prep_aborts_before_the_model_loads() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let supervisor = SidecarSupervisor::new();
    let record = record();
    let runtime =
        PythonSidecarRuntime::new(slow_prep_descriptor(), governor.clone(), supervisor.clone());

    {
        let mut stream = runtime.stream(&record, Capability::chat(), chat_payload("abcdef"));
        // Read the preparing status, so the driver is inside the slow env prep.
        let first = stream.recv().await.expect("preparing").expect("no error");
        assert!(matches!(first, CapabilityChunk::Status(_)));
        // Drop the stream mid-prep.
    }

    // With the consumer gone, the driver must abort during prep and never load
    // the model (env prep ~500ms; wait past it).
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        !governor.is_resident(&record.id),
        "an abandoned request never loads the model"
    );
}

#[tokio::test]
async fn dropping_the_stream_ends_the_generation() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let supervisor = SidecarSupervisor::new();
    let record = record();
    let descriptor = descriptor();
    let runtime_id = descriptor.runtime_id.clone();
    let runtime = PythonSidecarRuntime::new(descriptor, governor.clone(), supervisor.clone());

    {
        let mut stream = runtime.stream(&record, Capability::chat(), chat_payload("slow"));
        while let Some(item) = stream.recv().await {
            if let Ok(CapabilityChunk::Text(_)) = item {
                break;
            }
        }
        // drop the stream mid-generation
    }

    // The driver detects the dropped consumer, releases the gate, and ends the
    // generation lease.
    let mut balanced = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if governor.leases().count(&record.id) == 0 {
            balanced = true;
            break;
        }
    }
    assert!(
        balanced,
        "the generation lease ends after the stream is dropped"
    );

    supervisor.shutdown(&runtime_id).await;
}
