//! Tests for the governed load helpers: `warm_load_acquire` (admit → load →
//! resident → producer lease, driving a real fake sidecar) and
//! `governed_one_shot` (gate + generation-lease bracketing).

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::records::{Capability, Modality, ModelRecord, ModelSource, SourceKind};
use runtime::governed::{governed_one_shot, warm_load_acquire};
use runtime::governor::{GovernorConfig, GpuProducer, MemoryGovernor};
use runtime::sidecar::{SidecarSpec, SidecarSupervisor};
use tokio::time::timeout;

const SHORT: Duration = Duration::from_millis(50);
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn record(footprint_mb: Option<i64>) -> ModelRecord {
    let mut record = ModelRecord::new(
        "Test Model",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/tmp/test-model"),
    );
    record.footprint_mb = footprint_mb;
    record
}

fn fake_spec() -> SidecarSpec {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_sidecar.py").to_owned();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut spec = SidecarSpec::new(
        format!("warm-{unique}"),
        PathBuf::from("/usr/bin/env"),
        vec!["python3".to_owned(), script, "normal".to_owned()],
    );
    spec.ready_timeout = Duration::from_secs(15);
    spec
}

#[tokio::test]
async fn warm_load_admits_loads_and_returns_the_producer_lease() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let supervisor = SidecarSupervisor::new();
    let record = record(Some(2048));
    let spec = fake_spec();
    let statuses = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&statuses);
    let status = move |message: &str| sink.lock().expect("lock").push(message.to_owned());

    let guard = warm_load_acquire(
        &governor,
        &supervisor,
        &spec,
        &record,
        GpuProducer::Generation(record.id.clone()),
        None,
        "Starting the runtime…",
        &status,
    )
    .await
    .expect("warm load");

    assert!(
        supervisor.is_running(&spec.runtime_id),
        "the sidecar loaded"
    );
    assert!(
        governor.is_resident(&record.id),
        "the model is a governed resident"
    );
    assert!(
        statuses
            .lock()
            .expect("lock")
            .iter()
            .any(|s| s == "Starting the runtime…"),
        "the starting status was reported"
    );

    // A second acquire while it is already running skips the load and returns
    // immediately (identical Generation producers co-hold the gate).
    let guard2 = timeout(
        SHORT,
        warm_load_acquire(
            &governor,
            &supervisor,
            &spec,
            &record,
            GpuProducer::Generation(record.id.clone()),
            None,
            "Starting the runtime…",
            &status,
        ),
    )
    .await
    .expect("second acquire is fast")
    .expect("warm load");
    assert!(governor.is_resident(&record.id));

    drop(guard);
    drop(guard2);
    supervisor.shutdown(&spec.runtime_id).await;
}

#[tokio::test]
async fn cancelling_a_warm_load_mid_flight_leaves_no_phantom_resident() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let supervisor = SidecarSupervisor::new();
    let record = record(Some(2048));
    // A sidecar that never becomes ready, so `ensure_running` blocks and the
    // warm load can be aborted mid-flight (after `admit` reserved the model).
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_sidecar.py").to_owned();
    let mut spec = SidecarSpec::new(
        "warm-cancel".to_owned(),
        PathBuf::from("/usr/bin/env"),
        vec!["python3".to_owned(), script, "never-ready".to_owned()],
    );
    spec.ready_timeout = Duration::from_secs(30);

    let task = {
        let (governor, supervisor, record, spec) = (
            governor.clone(),
            supervisor.clone(),
            record.clone(),
            spec.clone(),
        );
        tokio::spawn(async move {
            let status = |_: &str| {};
            let _ = warm_load_acquire(
                &governor,
                &supervisor,
                &spec,
                &record,
                GpuProducer::Generation(record.id.clone()),
                None,
                "Starting…",
                &status,
            )
            .await;
        })
    };

    // Let admit reserve the model and ensure_running start (and block on ready).
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        governor.is_resident(&record.id),
        "the model is reserved during the load"
    );
    task.abort();
    let _ = task.await;

    assert!(
        !governor.is_resident(&record.id),
        "cancelling the load unloads the reservation — no phantom resident"
    );
    supervisor.terminate_all();
}

#[tokio::test]
async fn governed_one_shot_holds_the_gate_and_balances_the_lease() {
    let governor = MemoryGovernor::new(GovernorConfig::with_total_mb(262_144));
    let record = record(Some(100));
    let status = |_: &str| {};

    let result: Result<i32, ()> = governed_one_shot(
        &governor,
        &record,
        GpuProducer::Generation(record.id.clone()),
        &status,
        async {
            // The producer gate is held for the whole body, so a different
            // exclusive producer cannot acquire it.
            let blocked = timeout(
                SHORT,
                governor
                    .gate()
                    .acquire(GpuProducer::Load("other".to_owned())),
            )
            .await;
            assert!(blocked.is_err(), "the gate is held during the body");
            Ok(1)
        },
    )
    .await;

    assert_eq!(result, Ok(1));
    assert_eq!(
        governor.leases().count(&record.id),
        0,
        "the generation lease is balanced"
    );
    assert!(
        timeout(
            SHORT,
            governor
                .gate()
                .acquire(GpuProducer::Load("other".to_owned()))
        )
        .await
        .is_ok(),
        "the gate is free after the body"
    );
}
