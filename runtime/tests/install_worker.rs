//! Tests for the pull worker: what it writes into a job's record, how it
//! honours a control file, when it retries, how the slots cap concurrency, and
//! how two workers stay off each other's jobs. Driven by a scriptable provider,
//! so nothing here touches the network.

mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{PullControl, PullState, PullStore};
use kernel::install::{
    InstallAvailability, InstallError, InstallPlan, InstallProgress, InstallSearchHit,
    InstallStreamEvent,
};
use kernel::records::SourceKind;
use runtime::install::InstallService;
use runtime::install::provider::{InstallEventStream, InstallFuture, InstallProvider};
use runtime::install::worker::{PullWorker, RetryPolicy, SlotPool, WorkerError, claim_reference};
use runtime::settings::PullSettings;
use support::TempDir;
use tokio::sync::mpsc;

/// What one install attempt does. Progress is cumulative, as a real provider's
/// is: the first figure is what was already on disk when the attempt started.
#[derive(Clone)]
enum Behavior {
    /// Report the bytes already there, then finish.
    Lands,
    /// Report the bytes already there, then fail without moving a new one.
    FailsCold(InstallError),
    /// Report the bytes already there, transfer more, then fail.
    FailsWarm(InstallError),
    /// Say something, then finish.
    Says(String),
    /// Report progress, then hold the stream open until it is cancelled.
    Hangs,
}

struct MockProvider {
    /// One behavior per attempt; the last repeats once the script runs out.
    script: Mutex<Vec<Behavior>>,
    attempts: AtomicU32,
    plan_error: Option<InstallError>,
}

impl MockProvider {
    fn new(script: Vec<Behavior>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script),
            attempts: AtomicU32::new(0),
            plan_error: None,
        })
    }

    fn refusing_to_plan(error: InstallError) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(vec![Behavior::Lands]),
            attempts: AtomicU32::new(0),
            plan_error: Some(error),
        })
    }

    fn attempts(&self) -> u32 {
        self.attempts.load(Ordering::Relaxed)
    }

    fn next(&self) -> Behavior {
        let mut script = self
            .script
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match script.len() {
            0 | 1 => script.first().cloned().unwrap_or(Behavior::Lands),
            _ => script.remove(0),
        }
    }
}

fn plan(reference: &str) -> InstallPlan {
    let mut plan = InstallPlan::new(
        InstallProviderId::huggingface(),
        reference,
        reference.rsplit('/').next().unwrap_or(reference),
        "/models/somewhere",
    );
    plan.total_bytes = Some(1_000);
    plan.remaining_bytes = Some(1_000);
    plan
}

impl InstallProvider for MockProvider {
    fn id(&self) -> InstallProviderId {
        InstallProviderId::huggingface()
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn source_kind(&self) -> SourceKind {
        SourceKind::huggingface_cache()
    }
    fn supports_search(&self) -> bool {
        false
    }
    fn availability(&self) -> InstallFuture<'_, InstallAvailability> {
        Box::pin(async { InstallAvailability::Ready })
    }
    fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> InstallFuture<'_, Result<Vec<InstallSearchHit>, InstallError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn plan(&self, reference: &str) -> InstallFuture<'_, Result<InstallPlan, InstallError>> {
        let reference = reference.to_owned();
        let error = self.plan_error.clone();
        Box::pin(async move {
            match error {
                Some(error) => Err(error),
                None => Ok(plan(&reference)),
            }
        })
    }
    fn install(&self, _plan: InstallPlan) -> InstallEventStream {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        let behavior = self.next();
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            if tx
                .send(Ok(InstallStreamEvent::Progress(progress_at(400))))
                .await
                .is_err()
            {
                return;
            }
            match behavior {
                Behavior::Lands => {}
                Behavior::Says(text) => {
                    let _ = tx.send(Ok(InstallStreamEvent::Status(text))).await;
                }
                Behavior::FailsCold(error) => {
                    let _ = tx.send(Err(error)).await;
                }
                Behavior::FailsWarm(error) => {
                    let _ = tx
                        .send(Ok(InstallStreamEvent::Progress(progress_at(800))))
                        .await;
                    let _ = tx.send(Err(error)).await;
                }
                Behavior::Hangs => tx.closed().await,
            }
        });
        rx
    }
}

fn progress_at(bytes: i64) -> InstallProgress {
    InstallProgress {
        bytes_downloaded: bytes,
        total_bytes: Some(1_000),
        total_is_partial: false,
        current_file: Some("model.gguf".to_owned()),
    }
}

fn service(provider: Arc<MockProvider>) -> InstallService {
    InstallService::new(vec![provider])
}

fn settings(max_concurrent: i64) -> PullSettings {
    PullSettings {
        max_concurrent,
        ..PullSettings::default()
    }
}

/// A worker over `provider`, coordinating through `store`.
fn worker(provider: Arc<MockProvider>, store: &PullStore, slots: i64) -> PullWorker {
    PullWorker::new(service(provider), store.root(), &settings(slots))
}

/// A policy that retries immediately, so a test is not a stopwatch.
fn brisk() -> RetryPolicy {
    RetryPolicy::new(vec![Duration::from_millis(1)], Duration::from_secs(60))
}

/// The retry lines a job recorded, oldest first.
fn retries(job: &kernel::install::pulls::PullJobDir) -> Vec<(u32, i64)> {
    job.events()
        .into_iter()
        .filter_map(|event| match event.kind {
            kernel::install::pulls::PullEventKind::Retry {
                attempt, delay_ms, ..
            } => Some((attempt, delay_ms)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_pull_that_lands_is_recorded_done_and_registered() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![Behavior::Lands]);
    let registered = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&registered);

    let worker = worker(provider, &store, 2).with_registrar(Arc::new(move || {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }));

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    let status = job.status();
    assert_eq!(status.state, PullState::Done);
    assert_eq!(status.progress.bytes_downloaded, 400);
    assert_eq!(status.pid, None);
    assert_eq!(registered.load(Ordering::Relaxed), 1);

    let states: Vec<PullState> = job
        .events()
        .into_iter()
        .filter_map(|event| match event.kind {
            kernel::install::pulls::PullEventKind::State { state } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![PullState::Queued, PullState::Running, PullState::Done]
    );
}

#[tokio::test]
async fn the_lock_is_free_once_the_worker_is_done() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2);

    worker.run(&job).await.unwrap();
    assert!(!job.worker_alive());
}

#[tokio::test]
async fn a_registration_that_fails_still_leaves_the_download_done() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2).with_registrar(
        Arc::new(|| Box::pin(async { Err("the registry would not open".to_owned()) })),
    );

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    assert_eq!(
        job.status().message.as_deref(),
        Some("the registry would not open")
    );
}

#[tokio::test]
async fn a_dropped_connection_is_tried_again() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![
        Behavior::FailsCold(InstallError::TransferFailed("connection reset".to_owned())),
        Behavior::FailsCold(InstallError::TransferFailed("connection reset".to_owned())),
        Behavior::Lands,
    ]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(brisk());

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    assert_eq!(provider.attempts(), 3);

    let retries = job
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind,
                kernel::install::pulls::PullEventKind::Retry { .. }
            )
        })
        .count();
    assert_eq!(retries, 2);
}

#[tokio::test]
async fn a_gated_repo_is_not_tried_again() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![Behavior::FailsCold(InstallError::AuthRequired(
        "org/Model".to_owned(),
    ))]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(brisk());

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Failed);
    assert_eq!(provider.attempts(), 1);
    assert!(
        job.status()
            .message
            .unwrap_or_default()
            .contains("is gated"),
        "the reason should survive into the record"
    );
}

#[tokio::test]
async fn a_reference_that_will_not_resolve_ends_the_job() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider =
        MockProvider::refusing_to_plan(InstallError::ReferenceNotFound("org/Model".to_owned()));
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(brisk());

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Failed);
    assert_eq!(provider.attempts(), 0);
}

#[tokio::test]
async fn a_streak_that_gets_nowhere_ends_the_job_interrupted() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![Behavior::FailsCold(InstallError::TransferFailed(
        "the network is gone".to_owned(),
    ))]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(RetryPolicy::new(
        vec![Duration::from_millis(20), Duration::from_millis(40)],
        Duration::from_millis(500),
    ));

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Interrupted);
    assert!(
        provider.attempts() > 1,
        "it gave up without retrying at all"
    );
    assert!(job.status().state.is_resumable());
    assert!(
        job.status()
            .message
            .unwrap_or_default()
            .contains("the network is gone")
    );

    // The waits grow while nothing new transfers.
    let recorded = retries(&job);
    assert_eq!(recorded.first().map(|(_, delay)| *delay), Some(20));
    assert_eq!(recorded.get(1).map(|(_, delay)| *delay), Some(40));
}

#[tokio::test]
async fn a_transfer_that_moved_bytes_starts_the_waits_over() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![
        Behavior::FailsWarm(InstallError::TransferFailed("dropped".to_owned())),
        Behavior::FailsWarm(InstallError::TransferFailed("dropped again".to_owned())),
        Behavior::Lands,
    ]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(RetryPolicy::new(
        vec![Duration::from_millis(5), Duration::from_millis(500)],
        Duration::from_secs(60),
    ));

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    let delays: Vec<i64> = retries(&job).into_iter().map(|(_, delay)| delay).collect();
    assert_eq!(
        delays,
        vec![5, 5],
        "a transfer that moved bytes should not be made to wait longer"
    );
}

#[tokio::test]
async fn a_job_waiting_to_retry_says_when_it_will() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![
        Behavior::FailsCold(InstallError::TransferFailed("reset".to_owned())),
        Behavior::Lands,
    ]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(RetryPolicy::new(
        vec![Duration::from_millis(400)],
        Duration::from_secs(60),
    ));

    let watched = job.clone();
    let watcher = tokio::spawn(async move {
        for _ in 0..200 {
            let status = watched.stored_status();
            if let Some(due) = status.next_attempt_at_ms {
                return Some((due, status.updated_at_ms));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        None
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    let (due, written) = watcher.await.unwrap().expect("a retry was announced");
    assert!(due > written, "the next attempt should be in the future");
    assert_eq!(job.status().next_attempt_at_ms, None, "cleared once it ran");
}

#[tokio::test]
async fn what_the_provider_says_reaches_the_record_and_the_history() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(
        MockProvider::new(vec![Behavior::Says("resolving 3 files".to_owned())]),
        &store,
        2,
    );

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    assert_eq!(
        job.status().status_line.as_deref(),
        Some("resolving 3 files")
    );
    assert!(job.events().into_iter().any(|event| matches!(
        event.kind,
        kernel::install::pulls::PullEventKind::Status { ref text } if text == "resolving 3 files"
    )));
}

#[tokio::test]
async fn a_reader_probing_the_lock_does_not_cost_the_worker_its_job() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    // A client polling `status()` holds a shared lock for an instant at a time;
    // the worker's claim has to ride that out rather than stand down.
    let probed = job.clone();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let until = Arc::clone(&stop);
    let prober = std::thread::spawn(move || {
        while !until.load(Ordering::Relaxed) {
            let _ = probed.status();
        }
    });

    let outcome = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2)
        .run(&job)
        .await;
    stop.store(true, Ordering::Relaxed);
    prober.join().expect("prober thread");
    assert_eq!(outcome.unwrap(), PullState::Done);
}

#[tokio::test]
async fn a_job_sleeping_between_attempts_gives_up_its_slot() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let waiting = store.create(&plan("org/Slow"), 1_000).unwrap();
    let other = store.create(&plan("org/Other"), 2_000).unwrap();

    let stalling = MockProvider::new(vec![Behavior::FailsCold(InstallError::TransferFailed(
        "reset".to_owned(),
    ))]);
    let slow = worker(Arc::clone(&stalling), &store, 1).with_policy(RetryPolicy::new(
        vec![Duration::from_millis(1_500)],
        Duration::from_secs(60),
    ));
    let quick = worker(MockProvider::new(vec![Behavior::Lands]), &store, 1);

    let sleeping = waiting.clone();
    let held = tokio::spawn(async move { slow.run(&sleeping).await });
    while waiting.stored_status().next_attempt_at_ms.is_none() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // The one slot must be free while the other job waits out its backoff.
    assert_eq!(quick.run(&other).await.unwrap(), PullState::Done);

    waiting.request(PullControl::Cancel).unwrap();
    held.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_worker_waits_for_a_slot_and_takes_it_when_one_frees_up() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let pool = SlotPool::new(store.root(), 1);
    let taken = pool.try_take().unwrap().expect("a free slot");

    let worker = worker(MockProvider::new(vec![Behavior::Lands]), &store, 1);
    let releasing = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        drop(taken);
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    releasing.await.unwrap();
}

#[tokio::test]
async fn a_cancel_stops_the_transfer_and_takes_its_control_with_it() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);

    let asking = job.clone();
    let asker = tokio::spawn(async move {
        loop {
            if asking.stored_status().state == PullState::Running {
                asking.request(PullControl::Cancel).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Cancelled);
    asker.await.unwrap();
    assert_eq!(job.control(), None);
    assert_eq!(job.status().state, PullState::Cancelled);
}

#[tokio::test]
async fn a_pause_leaves_the_job_resumable() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);

    let asking = job.clone();
    let asker = tokio::spawn(async move {
        loop {
            if asking.stored_status().state == PullState::Running {
                asking.request(PullControl::Pause).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Paused);
    asker.await.unwrap();
    assert!(job.status().state.is_resumable());
    assert_eq!(job.control(), None);
}

#[tokio::test]
async fn a_cancel_that_arrives_before_the_slot_does_is_still_honoured() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![Behavior::Lands]);
    // Every slot is taken, so the worker cannot start transferring.
    let pool = SlotPool::new(store.root(), 1);
    let _taken = pool.try_take().unwrap().expect("a free slot");

    let worker = worker(Arc::clone(&provider), &store, 1);
    let waiting = job.clone();
    let asker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(waiting.stored_status().state, PullState::Queued);
        waiting.request(PullControl::Cancel).unwrap();
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Cancelled);
    asker.await.unwrap();
    assert_eq!(provider.attempts(), 0, "it never started transferring");
}

#[tokio::test]
async fn a_second_worker_will_not_take_a_job_that_is_already_running() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let first = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);
    let second = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2);

    let running = job.clone();
    let held = tokio::spawn(async move { first.run(&running).await });
    while job.stored_status().state != PullState::Running {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(matches!(
        second.run(&job).await,
        Err(WorkerError::AlreadyRunning)
    ));

    job.request(PullControl::Cancel).unwrap();
    held.await.unwrap().unwrap();
}

#[tokio::test]
async fn two_jobs_for_one_reference_do_not_run_at_once() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let first = store.create(&plan("org/Model"), 1_000).unwrap();
    let second = store.create(&plan("org/Model"), 2_000).unwrap();
    let holder = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 4);
    let other = worker(MockProvider::new(vec![Behavior::Lands]), &store, 4);

    let running = first.clone();
    let held = tokio::spawn(async move { holder.run(&running).await });
    while first.stored_status().state != PullState::Running {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    match other.run(&second).await {
        Err(WorkerError::AlreadyPulling(reference)) => assert_eq!(reference, "org/Model"),
        other => panic!("expected the reference to be claimed, got {other:?}"),
    }
    // The refused job must not look like one whose worker died, or a client
    // with auto-resume on would respawn it against the same held reference.
    assert_eq!(second.status().state, PullState::Queued);
    assert!(
        second
            .status()
            .message
            .unwrap_or_default()
            .contains("already being pulled")
    );

    first.request(PullControl::Cancel).unwrap();
    held.await.unwrap().unwrap();
}

#[test]
fn a_reference_claim_is_released_when_it_is_dropped() {
    let dir = TempDir::new();
    let root = dir.join("pulls");
    let provider = InstallProviderId::huggingface();

    let first = claim_reference(&root, provider.as_str(), "org/Model")
        .unwrap()
        .expect("an unclaimed reference");
    assert!(
        claim_reference(&root, provider.as_str(), "org/Model")
            .unwrap()
            .is_none()
    );
    assert!(
        claim_reference(&root, provider.as_str(), "org/Other")
            .unwrap()
            .is_some(),
        "another reference is not blocked"
    );
    assert!(
        claim_reference(&root, "ollama", "org/Model")
            .unwrap()
            .is_some(),
        "the same name on another provider is not blocked"
    );

    drop(first);
    assert!(
        claim_reference(&root, provider.as_str(), "org/Model")
            .unwrap()
            .is_some()
    );
}

#[test]
fn the_slots_cap_what_can_run_and_free_up_after() {
    let dir = TempDir::new();
    let pool = SlotPool::new(dir.join("pulls"), 2);

    let first = pool.try_take().unwrap().expect("a free slot");
    let second = pool.try_take().unwrap().expect("a second free slot");
    assert!(pool.try_take().unwrap().is_none(), "the cap did not hold");

    drop(first);
    assert!(pool.try_take().unwrap().is_some());
    drop(second);
}

#[test]
fn a_pool_always_has_at_least_one_slot() {
    let dir = TempDir::new();
    let pool = SlotPool::new(dir.join("pulls"), 0);
    assert!(pool.try_take().unwrap().is_some());
}

#[test]
fn the_backoff_grows_and_then_holds() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.delay(1), Duration::from_secs(5));
    assert_eq!(policy.delay(2), Duration::from_secs(15));
    assert_eq!(policy.delay(5), Duration::from_secs(300));
    assert_eq!(policy.delay(50), Duration::from_secs(300));
    assert_eq!(policy.delay(0), Duration::from_secs(5));
}

#[test]
fn only_the_network_is_worth_retrying() {
    assert!(RetryPolicy::retryable(&InstallError::TransferFailed(
        "reset".into()
    )));
    assert!(RetryPolicy::retryable(&InstallError::ProviderUnavailable(
        "the daemon is not running".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::AuthRequired(
        "org/M".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::ChecksumMismatch(
        "f".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::ReferenceNotFound(
        "org/M".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::ReferenceInvalid(
        "??".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::InsufficientDisk {
        required_bytes: 10,
        available_bytes: 1
    }));
}
