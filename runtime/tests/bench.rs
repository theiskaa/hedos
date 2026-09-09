//! The bench driver end to end against a fake adapter: the eviction either side
//! of a model's turn, the cold run, the warm runs, and what a failure or a stop
//! does to the walk.

mod support;

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use kernel::Registry;
use kernel::artifacts::ArtifactStore;
use kernel::bench::{ColdStart, Phase, Status};
use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::jobs::JobHistoryStore;
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{ChunkStream, RuntimeAdapter, RuntimeError};
use runtime::bench::{BenchEvent, BenchPlan, Cancel, EvictFuture, Eviction, Prepare};
use runtime::facade::{Kernel, RegisteredAdapter};
use runtime::governor::{GovernorConfig, MemoryGovernor};
use support::TempDir;
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// An adapter that answers with a fixed reply and reported stats, or fails.
struct Fake {
    id: RuntimeId,
    /// Model ids that fail instead of answering.
    failing: HashSet<String>,
    /// How many times each model was invoked.
    calls: Arc<StdMutex<Vec<String>>>,
    /// Stop the bench once this many runs have been asked for, which is how a
    /// stop lands part way through a model's turn.
    stop_after: Option<(usize, Cancel)>,
}

impl Fake {
    fn new() -> Self {
        Self {
            id: RuntimeId::ollama(),
            failing: HashSet::new(),
            calls: Arc::new(StdMutex::new(Vec::new())),
            stop_after: None,
        }
    }

    fn stopping_after(mut self, runs: usize, cancel: &Cancel) -> Self {
        self.stop_after = Some((runs, cancel.clone()));
        self
    }

    fn failing(mut self, model_id: &str) -> Self {
        self.failing.insert(model_id.to_owned());
        self
    }
}

impl RuntimeAdapter for Fake {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, _record: &ModelRecord, capability: &Capability) -> bool {
        *capability == Capability::chat()
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        _capability: Capability,
        _payload: JsonValue,
    ) -> ChunkStream {
        let runs = {
            let mut calls = self.calls.lock().expect("lock");
            calls.push(record.id.clone());
            calls.len()
        };
        if let Some((after, cancel)) = &self.stop_after
            && runs >= *after
        {
            cancel.stop();
        }
        let (tx, stream) = ChunkStream::channel();
        if self.failing.contains(&record.id) {
            let _ = tx.send(Err(RuntimeError::Unavailable(
                "the backend is down".to_owned(),
            )));
            return stream;
        }
        let _ = tx.send(Ok(CapabilityChunk::Text("hello there".to_owned())));
        let _ = tx.send(Ok(CapabilityChunk::Done(Some(GenerationStats {
            completion_tokens: Some(20),
            eval_ms: Some(500),
            ..GenerationStats::default()
        }))));
        stream
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

/// A [`Prepare`] that records what it was asked to clear and answers as told.
struct FakePrepare {
    asked: Arc<StdMutex<Vec<String>>>,
    held: Option<String>,
}

impl FakePrepare {
    fn clearing() -> Self {
        Self {
            asked: Arc::new(StdMutex::new(Vec::new())),
            held: None,
        }
    }

    fn holding(by: &str) -> Self {
        Self {
            asked: Arc::new(StdMutex::new(Vec::new())),
            held: Some(by.to_owned()),
        }
    }
}

impl Prepare for FakePrepare {
    fn evict(&self, model_id: &str) -> EvictFuture {
        self.asked.lock().expect("lock").push(model_id.to_owned());
        let held = self.held.clone();
        Box::pin(async move {
            match held {
                Some(holder) => Eviction::Held(holder),
                None => Eviction::Cleared,
            }
        })
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

fn kernel(dir: &TempDir, ids: &[&str], adapter: Fake) -> Kernel {
    let mut registry = Registry::open(dir.path()).expect("registry");
    for id in ids {
        registry.register(record(id)).expect("register");
    }
    let artifacts = ArtifactStore::new(dir.path());
    let governor = Arc::new(MemoryGovernor::new(GovernorConfig::with_total_mb(262_144)));
    let history = JobHistoryStore::with_default_limit(dir.path());
    Kernel::new(
        registry,
        artifacts,
        governor,
        history,
        vec![RegisteredAdapter::streaming(Arc::new(adapter))],
    )
}

fn plan(ids: &[&str], runs: usize) -> BenchPlan {
    let mut plan = BenchPlan::new(ids.iter().map(|id| (*id).to_owned()).collect());
    plan.runs = runs;
    plan
}

/// Every event, drained after the run.
fn drain(mut rx: UnboundedReceiver<BenchEvent>) -> Vec<BenchEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

/// The settled status of one model.
fn settled(events: &[BenchEvent], id: &str) -> Status {
    events
        .iter()
        .find_map(|event| match event {
            BenchEvent::Settled {
                id: settled,
                status,
            } if settled == id => Some((**status).clone()),
            _ => None,
        })
        .expect("a settled status")
}

#[tokio::test]
async fn a_model_is_cleared_before_its_turn_and_again_after() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["m"], Fake::new());
    let prepare = FakePrepare::clearing();
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(&kernel, &plan(&["m"], 2), &prepare, &Cancel::new(), &tx).await;

    assert_eq!(
        *prepare.asked.lock().expect("lock"),
        vec!["m".to_owned(), "m".to_owned()],
        "cleared before the cold run and again after the warm ones"
    );
    let events = drain(rx);
    match settled(&events, "m") {
        Status::Done(figures) => {
            assert!(matches!(figures.cold_start, ColdStart::Measured(_)));
            assert_eq!(figures.runs.len(), 2, "one sample per warm run");
            assert_eq!(
                figures.tokens_per_second.expect("a rate").median,
                40.0,
                "20 tokens in the 500ms the backend reported"
            );
        }
        other => panic!("expected figures, got {other:?}"),
    }
}

#[tokio::test]
async fn a_model_something_else_holds_reports_why_it_has_no_cold_figure() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["m"], Fake::new());
    let prepare = FakePrepare::holding("a running gateway");
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(&kernel, &plan(&["m"], 1), &prepare, &Cancel::new(), &tx).await;

    match settled(&drain(rx), "m") {
        Status::Done(figures) => {
            assert_eq!(
                figures.cold_start,
                ColdStart::Held("a running gateway".to_owned()),
                "the warm figures are still real, the cold one says who holds it"
            );
            assert!(figures.tokens_per_second.is_some());
        }
        other => panic!("expected figures, got {other:?}"),
    }
}

#[tokio::test]
async fn keeping_models_warm_skips_both_the_eviction_and_the_cold_run() {
    let dir = TempDir::new();
    let fake = Fake::new();
    let calls = Arc::clone(&fake.calls);
    let kernel = kernel(&dir, &["m"], fake);
    let prepare = FakePrepare::clearing();
    let mut plan = plan(&["m"], 2);
    plan.keep_warm = true;
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(&kernel, &plan, &prepare, &Cancel::new(), &tx).await;

    assert!(
        prepare.asked.lock().expect("lock").is_empty(),
        "residency is left alone"
    );
    assert_eq!(calls.lock().expect("lock").len(), 2, "only the warm runs");
    match settled(&drain(rx), "m") {
        Status::Done(figures) => assert_eq!(figures.cold_start, ColdStart::NotMeasured),
        other => panic!("expected figures, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failure_is_one_row_and_the_walk_carries_on() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["broken", "fine"], Fake::new().failing("broken"));
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["broken", "fine"], 1),
        &FakePrepare::clearing(),
        &Cancel::new(),
        &tx,
    )
    .await;

    let events = drain(rx);
    match settled(&events, "broken") {
        Status::Failed(reason) => assert!(reason.contains("down"), "{reason}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(
        matches!(settled(&events, "fine"), Status::Done(_)),
        "the model after it is still benched"
    );
}

#[tokio::test]
async fn a_model_that_is_not_registered_fails_rather_than_ending_the_bench() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["fine"], Fake::new());
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["ghost", "fine"], 1),
        &FakePrepare::clearing(),
        &Cancel::new(),
        &tx,
    )
    .await;

    let events = drain(rx);
    assert!(matches!(settled(&events, "ghost"), Status::Failed(_)));
    assert!(matches!(settled(&events, "fine"), Status::Done(_)));
}

#[tokio::test]
async fn a_stop_settles_the_models_it_never_reached() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["a", "b"], Fake::new());
    let cancel = Cancel::new();
    cancel.stop();
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["a", "b"], 1),
        &FakePrepare::clearing(),
        &cancel,
        &tx,
    )
    .await;

    let events = drain(rx);
    assert!(matches!(settled(&events, "a"), Status::Stopped));
    assert!(matches!(settled(&events, "b"), Status::Stopped));
}

#[tokio::test]
async fn the_cold_run_is_announced_before_the_warm_ones() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["m"], Fake::new());
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["m"], 2),
        &FakePrepare::clearing(),
        &Cancel::new(),
        &tx,
    )
    .await;

    let phases: Vec<Phase> = drain(rx)
        .into_iter()
        .filter_map(|event| match event {
            BenchEvent::Started { phase, .. } => Some(phase),
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            Phase::ColdStart,
            Phase::Warm { run: 1, of: 2 },
            Phase::Warm { run: 2, of: 2 },
        ]
    );
}

#[tokio::test]
async fn a_model_is_put_back_down_even_when_a_run_fails() {
    let dir = TempDir::new();
    let kernel = kernel(&dir, &["broken"], Fake::new().failing("broken"));
    let prepare = FakePrepare::clearing();
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["broken"], 1),
        &prepare,
        &Cancel::new(),
        &tx,
    )
    .await;

    assert!(matches!(settled(&drain(rx), "broken"), Status::Failed(_)));
    assert_eq!(
        prepare.asked.lock().expect("lock").len(),
        2,
        "cleared before the run and again after it failed, so the next model \
         does not measure against weights this one left in memory"
    );
}

#[tokio::test]
async fn a_stop_part_way_through_keeps_what_was_measured() {
    let dir = TempDir::new();
    let cancel_at = Cancel::new();
    // The cold run and the first warm one land; the stop is asked for as the
    // second warm run opens, so that one never counts.
    let kernel = kernel(&dir, &["m"], Fake::new().stopping_after(3, &cancel_at));
    let (tx, rx) = mpsc::unbounded_channel();

    runtime::bench::run(
        &kernel,
        &plan(&["m"], 3),
        &FakePrepare::clearing(),
        &cancel_at,
        &tx,
    )
    .await;

    match settled(&drain(rx), "m") {
        Status::Done(figures) => {
            assert!(matches!(figures.cold_start, ColdStart::Measured(_)));
            assert!(!figures.runs.is_empty(), "the warm run that landed stands");
            assert!(figures.runs.len() < 3, "and the rest never ran");
        }
        other => panic!("expected the figures it had, got {other:?}"),
    }
}
