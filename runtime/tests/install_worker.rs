//! Tests for the pull worker: what it writes into a job's record, how it
//! honours a control file, when it retries, how the slots cap concurrency, and
//! how two workers stay off each other's jobs. Driven by a scriptable provider,
//! so nothing here touches the network.

mod support;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{
    PullControl, PullError, PullEvent, PullEventKind, PullState, PullStore, REGISTERING_LINE,
    StopAnswer,
};
use kernel::install::{
    InstallAvailability, InstallError, InstallPlan, InstallProgress, InstallSearchHit,
    InstallStreamEvent,
};
use kernel::records::SourceKind;
use runtime::install::InstallService;
use runtime::install::provider::{InstallEventStream, InstallFuture, InstallProvider};
use runtime::install::worker::{
    PullWorker, RetryPolicy, SlotPool, Started, Stopped, WorkerError, await_answer,
    claim_reference, collect_ended, restart, restart_within, resume_all, start_or_join, stop,
    sweep_claims,
};
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
    /// Say something, transfer more, then hold the stream open until it is
    /// cancelled.
    SaysThenMoves(String),
    /// Report progress, then hold the stream open until it is cancelled.
    Hangs,
    /// Say something, then hold the stream open until it is cancelled.
    SaysThenHangs(String),
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
                Behavior::SaysThenMoves(text) => {
                    let _ = tx.send(Ok(InstallStreamEvent::Status(text))).await;
                    let _ = tx
                        .send(Ok(InstallStreamEvent::Progress(progress_at(800))))
                        .await;
                    tx.closed().await;
                }
                Behavior::Hangs => tx.closed().await,
                Behavior::SaysThenHangs(text) => {
                    let _ = tx.send(Ok(InstallStreamEvent::Status(text))).await;
                    tx.closed().await;
                }
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

    let during = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&during);
    let registering = job.clone();
    let worker = worker(provider, &store, 2).with_registrar(Arc::new(move || {
        let counter = Arc::clone(&counter);
        let seen = Arc::clone(&seen);
        let registering = registering.clone();
        Box::pin(async move {
            *seen.lock().unwrap() = Some(registering.stored_status());
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }));

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    let status = job.status();
    assert_eq!(status.state, PullState::Done);
    // While the store was scanned the record said so and was past stopping;
    // not after.
    let during = during.lock().unwrap().clone().expect("the registrar ran");
    assert_eq!(during.status_line.as_deref(), Some("registering"));
    assert!(during.registering_until_ms.is_some());
    assert!(during.past_stopping());
    assert_eq!(status.status_line, None, "over with the job");
    assert_eq!(status.registering_until_ms, None);
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
async fn a_pause_that_lands_while_the_store_is_scanned_is_written_down_not_swallowed() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let asking = job.clone();
    // The one stretch of a job that reads no control file: every byte has
    // landed and the store is being scanned.
    let worker = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2).with_registrar(
        Arc::new(move || {
            let asking = asking.clone();
            Box::pin(async move {
                asking.request(PullControl::Pause).unwrap();
                Ok(())
            })
        }),
    );

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    assert_eq!(job.status().state, PullState::Done);
    assert_eq!(
        job.control(),
        None,
        "the ask is not left for the next worker"
    );
    let said: Vec<String> = job
        .events()
        .into_iter()
        .filter_map(|event| match event.kind {
            kernel::install::pulls::PullEventKind::Status { text } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        said.iter()
            .any(|line| line == "pause arrived after every byte had landed"),
        "the history accounts for the ask: {said:?}"
    );
}

#[tokio::test]
async fn stopping_a_pull_past_stopping_is_refused_rather_than_answered_with_a_promise() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let _held = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    // A worker holding the job while it registers what it fetched, and reading
    // no control file until it is done. Its mark alone shows the window: a
    // provider whose total is only an estimate never reports a full fraction.
    let until = kernel::time::now_millis() + 30_000;
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
        status.status_line = Some(REGISTERING_LINE.to_owned());
        status.registering_until_ms = Some(until);
    })
    .unwrap();

    for control in [PullControl::Pause, PullControl::Cancel] {
        match stop(&job, control) {
            Err(error @ WorkerError::PastStopping { within_secs }) => {
                assert!((29..=30).contains(&within_secs), "{within_secs}");
                assert!(error.to_string().ends_with("ends within 30s"), "{error}");
            }
            other => panic!("a registering pull should refuse a {control:?}, got {other:?}"),
        }
    }
    assert_eq!(job.control(), None);

    // A window already run out is still refused, at zero rather than a wait
    // in the past.
    job.update_status(2_500, |status| status.registering_until_ms = Some(0))
        .unwrap();
    assert!(matches!(
        stop(&job, PullControl::Pause),
        Err(WorkerError::PastStopping { within_secs: 0 })
    ));

    // A full byte count on its own does not: the transfer being over is not the
    // worker being deaf, and the figure can be a previous attempt's or a total
    // summed from a listing that left a file's size out. The ask is written.
    job.update_status(3_000, |status| {
        status.registering_until_ms = None;
        status.status_line = None;
        status.progress.total_bytes = Some(400);
        status.progress.bytes_downloaded = 400;
    })
    .unwrap();
    assert_eq!(
        stop(&job, PullControl::Pause).unwrap(),
        Stopped::Asked(PullState::Running)
    );
    assert_eq!(
        job.control(),
        Some(PullControl::Pause),
        "the ask is left for the worker that is still reading it"
    );
    assert_eq!(job.status().state, PullState::Running);
}

#[tokio::test]
async fn a_provider_that_says_registering_can_still_be_paused() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(
        MockProvider::new(vec![Behavior::SaysThenHangs(REGISTERING_LINE.to_owned())]),
        &store,
        2,
    );

    // The provider's word lands on the status line, but the worker is still
    // transferring and reading the control file, so the ask is taken.
    let asking = job.clone();
    let asker = tokio::spawn(async move {
        loop {
            let status = asking.stored_status();
            if status.state == PullState::Running
                && status.status_line.as_deref() == Some(REGISTERING_LINE)
            {
                assert!(!status.past_stopping());
                return stop(&asking, PullControl::Pause).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Paused);
    assert_eq!(asker.await.unwrap(), Stopped::Asked(PullState::Running));
    assert_eq!(job.status().state, PullState::Paused);
}

#[tokio::test]
async fn a_resumed_job_carrying_a_finished_attempts_figures_can_still_be_stopped() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    // A worker died with every byte landed, and the job was put back in the
    // queue carrying those figures. Its new worker reads the control file
    // before it takes a slot, so an ask still reaches it.
    let _held = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    job.update_status(2_000, |status| {
        status.state = PullState::Queued;
        status.pid = Some(std::process::id());
        status.progress.total_bytes = Some(400);
        status.progress.bytes_downloaded = 400;
    })
    .unwrap();

    assert_eq!(
        stop(&job, PullControl::Cancel).unwrap(),
        Stopped::Asked(PullState::Queued)
    );
    assert_eq!(job.control(), Some(PullControl::Cancel));
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

/// A worker over `provider` that waits `seconds` for its registration.
fn registering_within(provider: Arc<MockProvider>, store: &PullStore, seconds: i64) -> PullWorker {
    let settings = PullSettings {
        register_timeout_seconds: seconds,
        ..settings(2)
    };
    PullWorker::new(service(provider), store.root(), &settings)
}

/// The status lines a job's history holds, oldest first.
fn said(job: &kernel::install::pulls::PullJobDir) -> Vec<String> {
    job.events()
        .into_iter()
        .filter_map(|event| match event.kind {
            PullEventKind::Status { text } => Some(text),
            _ => None,
        })
        .collect()
}

// The clock is paused, so the five-second window passes the moment the worker
// has nothing else to do; the registrar is on a thread the clock does not hold.
#[tokio::test(start_paused = true)]
async fn a_registration_that_never_returns_is_cut_short_and_the_job_still_lands() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = registering_within(MockProvider::new(vec![Behavior::Lands]), &store, 5)
        .with_registrar(Arc::new(|| Box::pin(std::future::pending())));

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    let status = job.status();
    let cut_short = "registration did not finish within 5s; the next scan will pick the model up";
    assert_eq!(status.state, PullState::Done);
    assert_eq!(status.message.as_deref(), Some(cut_short));
    assert!(said(&job).iter().any(|line| line == cut_short));
    assert!(!job.worker_alive(), "the lock is free once the job settles");
}

#[tokio::test(start_paused = true)]
async fn a_registration_that_blocks_its_thread_is_still_bounded() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    // A scan stuck in a blocking call never yields, so a timeout polled on the
    // same thread would only be looked at once the call returned.
    let worker = registering_within(MockProvider::new(vec![Behavior::Lands]), &store, 5)
        .with_registrar(Arc::new(|| {
            Box::pin(async {
                std::thread::sleep(Duration::from_secs(5));
                Ok(())
            })
        }));

    let began = std::time::Instant::now();
    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);

    assert!(
        began.elapsed() < Duration::from_secs(2),
        "{:?}",
        began.elapsed()
    );
    assert!(
        job.status()
            .message
            .is_some_and(|message| message.starts_with("registration did not finish"))
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
async fn a_machine_that_refuses_is_not_tried_again() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let provider = MockProvider::new(vec![Behavior::FailsCold(InstallError::Local(
        "Permission denied (os error 13)".to_owned(),
    ))]);
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(brisk());

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Failed);
    assert_eq!(provider.attempts(), 1);
    assert!(
        job.status()
            .message
            .unwrap_or_default()
            .contains("Permission denied"),
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
    // The window is wide enough for two retries on a loaded machine, and the
    // last step repeats until it is spent.
    let worker = worker(Arc::clone(&provider), &store, 2).with_policy(RetryPolicy::new(
        vec![Duration::from_millis(20), Duration::from_millis(40)],
        Duration::from_millis(1_500),
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
    // The history keeps what was said; the record does not once the job
    // has ended, since it is what the job is doing.
    assert_eq!(job.status().status_line, None);
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
    let watched = job.clone();
    let releasing = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        // While it waits, the record says what for.
        assert_eq!(
            watched.stored_status().status_line.as_deref(),
            Some("waiting for a free slot")
        );
        drop(taken);
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    releasing.await.unwrap();
    assert_eq!(job.stored_status().status_line, None);
    assert!(job.events().into_iter().any(|event| matches!(
        event.kind,
        PullEventKind::Status { ref text } if text == "waiting for a free slot"
    )));
}

#[tokio::test]
async fn what_the_provider_said_comes_off_the_record_once_bytes_move() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(
        MockProvider::new(vec![Behavior::SaysThenMoves(
            "Resolving org/Model".to_owned(),
        )]),
        &store,
        2,
    );

    let watching = job.clone();
    let watcher = tokio::spawn(async move {
        loop {
            let status = watching.stored_status();
            if status.progress.bytes_downloaded >= 800 {
                // The line was written, then the bytes moved past it.
                assert!(watching.events().into_iter().any(|event| matches!(
                    event.kind,
                    PullEventKind::Status { ref text } if text == "Resolving org/Model"
                )));
                // The bytes and the clear are one write, but the watcher may
                // have read the record between the flush and this poll.
                for _ in 0..50 {
                    if watching.stored_status().status_line.is_none() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert_eq!(watching.stored_status().status_line, None);
                watching.request(PullControl::Cancel).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Cancelled);
    watcher.await.unwrap();
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
async fn a_pause_the_worker_honours_is_answered_as_honoured() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);

    let asking = job.clone();
    let client = tokio::spawn(async move {
        while asking.stored_status().state != PullState::Running {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            stop(&asking, PullControl::Pause).unwrap(),
            Stopped::Asked(PullState::Running)
        );
        await_answer(&asking, PullControl::Pause, Duration::from_secs(5))
            .await
            .unwrap()
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Paused);
    assert_eq!(
        client.await.unwrap(),
        Some(StopAnswer::Honoured(PullState::Paused))
    );
}

#[tokio::test]
async fn a_pause_overtaken_by_the_last_byte_is_answered_as_landed() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    // The ask lands while the store is scanned, as one that passed its check a
    // moment before the worker marked itself registering would.
    let asking = job.clone();
    let worker = worker(MockProvider::new(vec![Behavior::Lands]), &store, 2).with_registrar(
        Arc::new(move || {
            let asking = asking.clone();
            Box::pin(async move {
                asking.request(PullControl::Pause).unwrap();
                Ok(())
            })
        }),
    );
    let waiting = job.clone();
    let client = tokio::spawn(async move {
        await_answer(&waiting, PullControl::Pause, Duration::from_secs(5))
            .await
            .unwrap()
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Done);
    assert_eq!(client.await.unwrap(), Some(StopAnswer::Landed));
    assert_eq!(job.status().state, PullState::Done);
}

#[tokio::test]
async fn an_ask_nobody_reads_is_left_pending_after_the_window() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let _held = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();

    assert_eq!(
        stop(&job, PullControl::Pause).unwrap(),
        Stopped::Asked(PullState::Running)
    );
    assert_eq!(
        await_answer(&job, PullControl::Pause, Duration::from_millis(100))
            .await
            .unwrap(),
        None
    );
    assert_eq!(job.control(), Some(PullControl::Pause), "the ask stands");
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
    // The refused job is settled rather than left live. A record that still
    // reads queued would keep a client joining this job instead of the pull
    // that owns the reference, and nothing would ever collect it.
    let refused = second.status();
    assert_eq!(refused.state, PullState::Failed);
    assert!(refused.state.is_terminal());
    assert!(refused.pid.is_none());
    assert!(
        refused
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
fn a_policy_from_settings_gives_up_after_the_settings_window() {
    let policy = RetryPolicy::from_settings(&PullSettings {
        retry_window_minutes: 30,
        ..PullSettings::default()
    });
    assert!(!policy.spent(0, 30 * 60_000 - 1));
    assert!(policy.spent(0, 30 * 60_000));
}

#[test]
fn the_default_policy_gives_up_when_the_default_settings_say() {
    let default = RetryPolicy::default();
    let settings = PullSettings::default();
    let configured = RetryPolicy::from_settings(&settings);
    let window = settings.retry_window().as_millis() as i64;
    for now in [window - 1, window] {
        assert_eq!(default.spent(0, now), configured.spent(0, now));
    }
    assert!(!default.spent(0, window - 1));
    assert!(default.spent(0, window));
    for attempt in 1..=6 {
        assert_eq!(default.delay(attempt), configured.delay(attempt));
    }
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
    assert!(!RetryPolicy::retryable(&InstallError::Local(
        "Permission denied (os error 13)".into()
    )));
    assert!(!RetryPolicy::retryable(&InstallError::AccessDenied(
        "org/M was not found on the platform, or is private".into()
    )));
}

#[tokio::test]
async fn a_stopped_job_is_put_back_to_queued_before_its_new_worker_starts() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| {
        status.state = PullState::Interrupted;
        status.pid = Some(4_242);
        status.message = Some("connection reset".to_owned());
        status.next_attempt_at_ms = Some(9_000);
    })
    .unwrap();
    job.request(PullControl::Pause).unwrap();

    // The spawn runs the real binary, which is not built in this test's target,
    // so only the record it leaves behind is under test here.
    let _ = restart(&job);

    assert_eq!(job.control(), None, "the ask that stopped it is dropped");
    let status = job.stored_status();
    assert_eq!(status.pid, None);
    assert_eq!(status.message, None);
    assert_eq!(status.next_attempt_at_ms, None);
}

#[tokio::test]
async fn a_cancel_its_worker_never_read_is_honoured_by_a_resume_instead_of_wedging_the_job() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Interrupted)
        .unwrap();
    job.request(PullControl::Cancel).unwrap();

    match restart(&job) {
        Err(WorkerError::Cancelled) => {}
        other => panic!("the cancel should have been honoured, got {other:?}"),
    }
    assert_eq!(job.stored_status().state, PullState::Cancelled);
    assert!(matches!(
        job.events().last(),
        Some(PullEvent {
            kind: PullEventKind::State {
                state: PullState::Cancelled
            },
            ..
        })
    ));
}

#[tokio::test]
async fn a_resume_over_a_cancel_that_replaced_a_pause_settles_cancelled() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Paused)
        .unwrap();
    job.request(PullControl::Pause).unwrap();
    job.request(PullControl::Cancel).unwrap();

    match restart(&job) {
        Err(WorkerError::Cancelled) => {}
        other => panic!("the cancel should have been honoured, got {other:?}"),
    }
    let status = job.stored_status();
    assert_eq!(status.state, PullState::Cancelled);
    assert_eq!(status.pid, None, "no worker was put on it");
    assert!(!job.worker_alive());
}

#[tokio::test]
async fn a_cancel_behind_a_pause_ends_the_job_cancelled() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);

    // A pause the worker reads and starts honouring, and a cancel written over
    // it while the worker waits to clear it, as a request in flight would. The
    // control lock is taken before the pause is visible, so the worker cannot
    // clear the pause before the cancel is in place.
    let asking = job.clone();
    let asker = std::thread::spawn(move || {
        while asking.stored_status().state != PullState::Running {
            std::thread::sleep(Duration::from_millis(10));
        }
        let held = kernel::install::pulls::take_lock(&asking.path().join("control.lock"))
            .unwrap()
            .expect("nothing has been asked, so nothing holds the control lock");
        std::fs::write(asking.path().join("control"), b"pause").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(asking.path().join("control"), b"cancel").unwrap();
        drop(held);
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Cancelled);
    asker.join().expect("the asking thread");
    assert_eq!(job.status().state, PullState::Cancelled);
    assert_eq!(job.control(), None, "neither ask is left for a next worker");
}

#[tokio::test]
async fn two_clients_asking_in_the_same_instant_share_one_job() {
    let dir = TempDir::new();
    let root = dir.join("pulls");
    for round in 0..4 {
        let plan = plan(&format!("org/Model{round}"));
        // Each client on a blocking thread of the runtime, as the TUI runs
        // it: a spawn needs the runtime's signal driver to reap the child.
        let clients: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                let plan = plan.clone();
                tokio::task::spawn_blocking(move || {
                    let store = PullStore::new(root);
                    start_or_join(&store, &plan).expect("start or join")
                })
            })
            .collect();
        let mut outcomes = Vec::new();
        for client in clients {
            outcomes.push(client.await.expect("a client thread"));
        }
        let created: Vec<&Started> = outcomes
            .iter()
            .filter(|started| matches!(started, Started::Created(_)))
            .collect();
        assert_eq!(created.len(), 1, "round {round}: {outcomes:?}");
        let joined = outcomes
            .iter()
            .find_map(|started| match started {
                Started::Joined(id) => Some(id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("round {round}: one client should join: {outcomes:?}"));
        assert!(matches!(created[0], Started::Created(id) if *id == joined));
    }
    let store = PullStore::new(root);
    assert_eq!(store.list().len(), 4, "one job per model, never two");
}

#[test]
fn a_reference_is_claimed_whatever_its_case() {
    let dir = TempDir::new();
    let root = dir.join("pulls");
    let held = claim_reference(&root, "huggingface", "org/Model")
        .unwrap()
        .expect("free");
    assert!(
        claim_reference(&root, "huggingface", "ORG/model")
            .unwrap()
            .is_none(),
        "one repo on the hub is one claim here"
    );
    drop(held);
}

#[tokio::test]
async fn starting_a_pull_lets_the_reference_go_for_its_worker() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let started = start_or_join(&store, &plan("org/Model")).unwrap();
    assert!(matches!(started, Started::Created(_)));
    let claim = claim_reference(store.root(), "huggingface", "org/Model")
        .expect("claim")
        .expect("the client let the reference go before returning");
    drop(claim);
}

#[tokio::test]
async fn restarting_a_job_whose_worker_died_writes_the_interruption_down() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(4_242);
    })
    .unwrap();

    restart(&job).expect("restart");
    let states: Vec<PullState> = job
        .events()
        .into_iter()
        .filter_map(|event| match event.kind {
            PullEventKind::State { state } => Some(state),
            _ => None,
        })
        .collect();
    assert_eq!(states.first(), Some(&PullState::Interrupted));
    assert_eq!(job.stored_status().state, PullState::Queued);

    // A pull the user paused was not interrupted, and its history does not
    // say it was.
    let paused = store.create(&plan("org/Other"), 1_000).unwrap();
    paused
        .update_status(1_000, |status| status.state = PullState::Paused)
        .unwrap();
    restart(&paused).expect("restart");
    assert!(paused.events().into_iter().all(|event| {
        !matches!(
            event.kind,
            PullEventKind::State {
                state: PullState::Interrupted
            }
        )
    }));
}

#[test]
fn sweeping_claims_takes_the_free_ones_and_leaves_a_held_one() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let held = claim_reference(store.root(), "huggingface", "org/Held")
        .unwrap()
        .expect("free");
    drop(
        claim_reference(store.root(), "huggingface", "org/Free")
            .unwrap()
            .expect("free"),
    );
    drop(
        claim_reference(store.root(), "ollama", "free:latest")
            .unwrap()
            .expect("free"),
    );
    assert_eq!(sweep_claims(store.root()), 2);
    let left: Vec<_> = std::fs::read_dir(store.root().join("locks"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(left.len(), 1);
    drop(held);
    assert_eq!(sweep_claims(store.root()), 1);
    assert_eq!(sweep_claims(store.root()), 0);
    // Collection does the same on the way past.
    drop(
        claim_reference(store.root(), "huggingface", "org/Again")
            .unwrap()
            .expect("free"),
    );
    collect_ended(&store, &PullSettings::default());
    assert!(
        std::fs::read_dir(store.root().join("locks"))
            .unwrap()
            .flatten()
            .next()
            .is_none()
    );
}

#[tokio::test]
async fn asking_for_a_model_whose_stopped_pull_carries_a_cancel_starts_afresh() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Interrupted)
        .unwrap();
    job.request(PullControl::Cancel).unwrap();

    // The spawn runs this test binary, which exits at once; the records are
    // what is under test.
    let started = start_or_join(&store, &plan("org/Model")).unwrap();
    assert!(matches!(started, Started::Created(ref id) if id != job.id()));
    assert_eq!(job.stored_status().state, PullState::Cancelled);
    assert_eq!(store.list().len(), 2);
}

#[tokio::test]
async fn a_resume_never_writes_over_an_ending_written_after_its_probe() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    // A worker finished and let go, but the cancel it never read is still on
    // disk: the ending stands, and the cancel is not honoured over it.
    job.update_status(1_000, |status| status.state = PullState::Done)
        .unwrap();
    job.request(PullControl::Cancel).unwrap();

    match restart(&job) {
        Err(WorkerError::Ended(PullState::Done)) => {}
        other => panic!("a done job should stay done, got {other:?}"),
    }
    assert_eq!(job.stored_status().state, PullState::Done);
}

#[tokio::test]
async fn pulling_a_model_past_a_job_nobody_took_up_settles_that_job() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let abandoned = store.create(&plan("org/Model"), 1_000).unwrap();
    let other = store.create(&plan("org/Other"), 1_000).unwrap();

    // The spawn runs this test binary, which exits at once; the records are
    // what is under test.
    let started = start_or_join(&store, &plan("org/Model")).unwrap();
    assert!(matches!(started, Started::Created(ref id) if id != abandoned.id()));
    let settled = abandoned.stored_status();
    assert_eq!(settled.state, PullState::Failed);
    assert_eq!(
        settled.message.as_deref(),
        Some("no worker took it up; started again")
    );
    assert_eq!(other.stored_status().state, PullState::Queued);
}

#[tokio::test]
async fn resuming_everything_takes_up_a_job_nobody_ever_came_for() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let abandoned = store.create(&plan("org/Abandoned"), 1_000).unwrap();
    let fresh = store
        .create(&plan("org/Fresh"), kernel::time::now_millis())
        .unwrap();
    let paused = store.create(&plan("org/Paused"), 1_000).unwrap();
    paused
        .update_status(1_000, |status| status.state = PullState::Paused)
        .unwrap();

    // An abandoned job whose model is being pulled again beside it is left,
    // since its worker would only fail as a duplicate.
    let passed = store.create(&plan("org/Passed"), 1_000).unwrap();
    store
        .create(&plan("org/Passed"), kernel::time::now_millis())
        .unwrap();

    // The spawn runs this test binary, which exits at once; which jobs were
    // taken up is what is under test.
    let restarted: Vec<String> = resume_all(&store).into_iter().map(|(id, _)| id).collect();
    assert_eq!(restarted, [abandoned.id().to_owned()]);
    assert_eq!(fresh.stored_status().state, PullState::Queued);
    assert_eq!(paused.stored_status().state, PullState::Paused);
    assert_eq!(passed.stored_status().pid, None);
}

#[tokio::test]
async fn a_job_that_has_ended_is_not_started_again() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Done)
        .unwrap();

    match restart(&job) {
        Err(WorkerError::Ended(PullState::Done)) => {}
        other => panic!("a done job should not be resumed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_job_a_worker_still_holds_is_not_started_again() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let _held = job.claim().unwrap().expect("the lock is free");

    match restart(&job) {
        Err(WorkerError::AlreadyRunning) => {}
        other => panic!("a held job should not be resumed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_pull_of_a_model_already_going_joins_it_rather_than_starting_a_second() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let _held = job.claim().unwrap().expect("the lock is free");
    job.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();

    let started = start_or_join(&store, &plan("org/Model")).unwrap();

    assert_eq!(started, Started::Joined(job.id().to_owned()));
    // Two workers on one model would fight over the same half-written files.
    assert_eq!(store.jobs().unwrap().len(), 1);
}

#[tokio::test]
async fn asking_for_a_model_carries_on_the_pull_of_it_that_stopped() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Paused)
        .unwrap();

    // Asking for a model is an instruction, so the paused pull of it is the one
    // that answers, rather than a second job onto the same half-written files.
    let started = start_or_join(&store, &plan("org/Model")).unwrap();

    assert_eq!(started, Started::Resumed(job.id().to_owned()));
    assert_eq!(store.jobs().unwrap().len(), 1);
}

#[tokio::test]
async fn resuming_everything_leaves_the_pulls_the_user_paused_alone() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let cut_off = store.create(&plan("org/CutOff"), 1_000).unwrap();
    cut_off
        .update_status(1_000, |status| status.state = PullState::Interrupted)
        .unwrap();
    let paused = store.create(&plan("org/Paused"), 2_000).unwrap();
    paused
        .update_status(2_000, |status| status.state = PullState::Paused)
        .unwrap();
    let done = store.create(&plan("org/Done"), 3_000).unwrap();
    done.update_status(3_000, |status| status.state = PullState::Done)
        .unwrap();

    let resumed = resume_all(&store);

    // Nobody asked for any of these. A worker that died is picked back up; a
    // pause the user chose has to survive, or a pause cannot be kept at all.
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].0, cut_off.id());
    assert_eq!(paused.status().state, PullState::Paused);
    assert_eq!(done.status().state, PullState::Done);
}

/// A record damaged past decoding.
const UNREADABLE: &[u8] = b"{ not json";

/// A job of `reference` whose record is [`UNREADABLE`].
fn unreadable_job(store: &PullStore, reference: &str) -> kernel::install::pulls::PullJobDir {
    let job = store.create(&plan(reference), 1_000).unwrap();
    std::fs::write(job.path().join("status.json"), UNREADABLE).unwrap();
    job
}

fn record_of(job: &kernel::install::pulls::PullJobDir) -> Vec<u8> {
    std::fs::read(job.path().join("status.json")).unwrap()
}

#[tokio::test]
async fn restarting_an_unreadable_record_is_refused_and_leaves_it_in_place() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = unreadable_job(&store, "org/Model");
    job.request(PullControl::Pause).unwrap();

    match restart(&job) {
        Err(WorkerError::Unreadable) => {}
        other => panic!("an unreadable record should not be resumed, got {other:?}"),
    }
    assert_eq!(record_of(&job), UNREADABLE);
    assert_eq!(job.control(), Some(PullControl::Pause));
    assert!(job.events().is_empty());
}

#[tokio::test]
async fn stopping_an_unreadable_record_is_refused() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = unreadable_job(&store, "org/Model");

    for control in [PullControl::Pause, PullControl::Cancel] {
        match stop(&job, control) {
            Err(WorkerError::Unreadable) => {}
            other => panic!("an unreadable record should not be stopped, got {other:?}"),
        }
    }
    assert!(!job.path().join("control").exists());
    assert_eq!(record_of(&job), UNREADABLE);
}

#[tokio::test]
async fn resume_all_leaves_an_unreadable_record_alone() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = unreadable_job(&store, "org/Model");

    assert!(resume_all(&store).is_empty());
    assert_eq!(record_of(&job), UNREADABLE);
    assert!(job.events().is_empty());
}

#[tokio::test]
async fn pulling_a_model_past_an_unreadable_record_starts_a_new_job() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = unreadable_job(&store, "org/Model");

    // The spawn runs this test binary, which exits at once; the records are
    // what is under test.
    let started = start_or_join(&store, &plan("org/Model")).unwrap();

    assert!(matches!(started, Started::Created(ref id) if id != job.id()));
    assert_eq!(store.list().len(), 2);
    assert_eq!(record_of(&job), UNREADABLE);
    assert_eq!(job.stored_status().state, PullState::Unreadable);
}

#[tokio::test]
async fn stopping_says_whether_it_asked_a_worker_or_settled_the_record_itself() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));

    let held = store.create(&plan("org/Held"), 1_000).unwrap();
    let _worker = held.claim().unwrap().expect("the lock is free");
    held.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();
    // A caller cannot work this out by reading the record twice: a job that
    // moved from queued to running in between would read as settled.
    assert!(matches!(
        stop(&held, PullControl::Cancel).unwrap(),
        Stopped::Asked(_)
    ));
    assert_eq!(held.status().state, PullState::Running);

    let loose = store.create(&plan("org/Loose"), 2_000).unwrap();
    loose
        .update_status(2_000, |status| status.state = PullState::Paused)
        .unwrap();
    assert_eq!(
        stop(&loose, PullControl::Cancel).unwrap(),
        Stopped::Settled(PullState::Cancelled)
    );
}

#[test]
fn collecting_keeps_the_newest_ended_pulls_the_settings_ask_for() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    for (index, reference) in ["a/old", "b/newer", "c/newest"].iter().enumerate() {
        let job = store
            .create(&plan(reference), 1_000 + index as i64)
            .unwrap();
        job.update_status(2_000 + index as i64, |status| {
            status.state = PullState::Done
        })
        .unwrap();
    }
    let going = store.create(&plan("d/going"), 5_000).unwrap();
    going
        .update_status(5_000, |status| status.state = PullState::Running)
        .unwrap();

    let settings = PullSettings {
        keep_ended: 1,
        ..PullSettings::default()
    };
    assert_eq!(collect_ended(&store, &settings), 2);
    let left: Vec<String> = store
        .list()
        .iter()
        .map(|job| job.job().reference.clone())
        .collect();
    assert_eq!(left, ["c/newest", "d/going"]);
    assert_eq!(collect_ended(&store, &settings), 0);
}

#[tokio::test]
async fn a_cancel_written_after_the_worker_stopped_reading_is_settled_once_it_lets_go() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let held = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();

    assert_eq!(
        stop(&job, PullControl::Cancel).unwrap(),
        Stopped::Asked(PullState::Running)
    );
    // The worker was already honouring a pause it had cleared: it settles
    // paused without reading the cancel, then exits.
    job.update_status(3_000, |status| status.state = PullState::Paused)
        .unwrap();
    let worker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(held);
    });

    let answer = await_answer(&job, PullControl::Cancel, Duration::from_secs(5))
        .await
        .unwrap();
    worker.await.unwrap();

    assert_eq!(answer, Some(StopAnswer::Honoured(PullState::Cancelled)));
    assert_eq!(job.status().state, PullState::Cancelled);
}

#[test]
fn a_pause_behind_a_waiting_cancel_is_refused_and_leaves_the_cancel() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let _held = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    job.update_status(2_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .unwrap();

    stop(&job, PullControl::Cancel).unwrap();
    match stop(&job, PullControl::Pause) {
        Err(WorkerError::CancelWaiting) => {}
        other => panic!("the pause should have been refused, got {other:?}"),
    }
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_job_nobody_took_up_whose_model_was_pulled_again_is_neither_restarted_nor_stopped() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let abandoned = store.create(&plan("org/Model"), 1_000).unwrap();
    let again = store.create(&plan("org/Model"), 2_000).unwrap();
    again
        .update_status(2_000, |status| status.state = PullState::Done)
        .unwrap();

    match restart(&abandoned) {
        Err(WorkerError::Ended(PullState::Failed)) => {}
        other => panic!("the resume should have been refused, got {other:?}"),
    }
    match stop(&abandoned, PullControl::Cancel) {
        Err(WorkerError::Ended(PullState::Failed)) => {}
        other => panic!("the cancel should have been refused, got {other:?}"),
    }
    assert!(resume_all(&store).is_empty());
    // Written down as the failure it reads, with no ask left behind.
    let status = abandoned.stored_status();
    assert_eq!(status.state, PullState::Failed);
    assert_eq!(
        status.message.as_deref(),
        Some(kernel::install::pulls::PULLED_AGAIN_LINE)
    );
    assert_eq!(abandoned.control(), None);
    again.remove().unwrap();
    assert!(resume_all(&store).is_empty(), "it stays failed");
}

#[tokio::test]
async fn of_two_jobs_nobody_took_up_the_second_stays_failed_once_the_first_lands() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let first = store.create(&plan("org/Model"), 1_000).unwrap();
    let second = store.create(&plan("org/Model"), 1_100).unwrap();

    // The spawn runs this test binary, which exits at once; the first job is
    // left queued for a worker, which is what makes the second a duplicate.
    let resumed = resume_all(&store);
    assert_eq!(resumed.len(), 2, "both read interrupted when listed");
    assert!(resumed[0].1.is_ok(), "{resumed:?}");
    assert!(
        matches!(resumed[1].1, Err(WorkerError::Ended(PullState::Failed))),
        "{resumed:?}"
    );
    assert_eq!(second.stored_status().state, PullState::Failed);

    first
        .update_status(kernel::time::now_millis(), |status| {
            status.state = PullState::Done
        })
        .unwrap();
    assert_eq!(
        second.reading(kernel::time::now_millis()).status.state,
        PullState::Failed
    );
    first.remove().unwrap();
    assert!(resume_all(&store).is_empty());
    assert_eq!(second.stored_status().state, PullState::Failed);
}

#[tokio::test]
async fn a_restart_gives_up_on_a_control_lock_held_past_its_wait() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    job.update_status(1_000, |status| status.state = PullState::Paused)
        .unwrap();
    job.request(PullControl::Pause).unwrap();
    let held = kernel::install::pulls::take_lock(&job.path().join("control.lock"))
        .unwrap()
        .expect("the control lock is free");

    match restart_within(&job, Duration::from_millis(30)) {
        Err(WorkerError::Record(PullError::ControlBusy(_))) => {}
        other => panic!("the restart should have given up, got {other:?}"),
    }
    let status = job.stored_status();
    assert_eq!(
        (status.state, status.updated_at_ms),
        (PullState::Paused, 1_000)
    );
    assert_eq!(job.control(), Some(PullControl::Pause));
    assert!(!job.worker_alive(), "the job's own lock was let go");
    drop(held);
}

#[tokio::test]
async fn resuming_everything_skips_a_job_whose_control_file_is_held_and_takes_it_next_time() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let held = kernel::install::pulls::take_lock(&job.path().join("control.lock"))
        .unwrap()
        .expect("the control lock is free");

    let started = std::time::Instant::now();
    let resumed = resume_all(&store);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "nobody waits on a busy job"
    );
    assert!(
        matches!(
            resumed.as_slice(),
            [(_, Err(WorkerError::Record(PullError::ControlBusy(_))))]
        ),
        "{resumed:?}"
    );
    assert_eq!(job.stored_status().updated_at_ms, 1_000);

    drop(held);
    let resumed = resume_all(&store);
    assert!(matches!(resumed.as_slice(), [(_, Ok(_))]), "{resumed:?}");
}

#[tokio::test]
async fn a_pause_written_over_a_cancel_being_honoured_goes_with_it() {
    let dir = TempDir::new();
    let store = PullStore::new(dir.join("pulls"));
    let job = store.create(&plan("org/Model"), 1_000).unwrap();
    let worker = worker(MockProvider::new(vec![Behavior::Hangs]), &store, 2);

    // An older build writes a pause over the cancel while the worker waits to
    // clear the cancel; the control lock is taken before the cancel is
    // visible, so the pause is in place before the clear.
    let asking = job.clone();
    let asker = std::thread::spawn(move || {
        while asking.stored_status().state != PullState::Running {
            std::thread::sleep(Duration::from_millis(10));
        }
        let held = kernel::install::pulls::take_lock(&asking.path().join("control.lock"))
            .unwrap()
            .expect("nothing has been asked, so nothing holds the control lock");
        std::fs::write(asking.path().join("control"), b"cancel").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        std::fs::write(asking.path().join("control"), b"pause").unwrap();
        drop(held);
    });

    assert_eq!(worker.run(&job).await.unwrap(), PullState::Cancelled);
    asker.join().expect("the asking thread");
    assert_eq!(job.control(), None, "no ask is left for a next worker");
}
