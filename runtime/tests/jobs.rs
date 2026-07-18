//! Integration tests for the `JobScheduler`: single-at-a-time execution, visible
//! queued waits, cancel (queued and mid-run), progress/preview streaming,
//! history persistence, and artifact-sink persistence.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use kernel::jobs::{JobEvent, JobHistoryStore, JobRuntimeEvent, JobState};
use kernel::records::{Capability, JsonValue};
use runtime::governor::{BoxFuture, RamVerdict};
use runtime::jobs::{ArtifactWriting, JobAdmission, JobError, JobScheduler, OnWait, Runner};
use tokio::sync::{Notify, mpsc};

use support::TempDir;

fn scheduler(dir: &TempDir) -> JobScheduler {
    JobScheduler::with_history(JobHistoryStore::with_default_limit(dir.path()))
}

fn image_runner(model_id: &str, total_steps: i64, step_delay: Duration) -> Runner {
    image_runner_with_cleanup(model_id, total_steps, step_delay, None)
}

fn image_runner_with_cleanup(
    model_id: &str,
    total_steps: i64,
    step_delay: Duration,
    cleanup: Option<Arc<AtomicBool>>,
) -> Runner {
    let model_id = model_id.to_owned();
    Box::new(move || {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _ = tx.send(Ok(JobRuntimeEvent::Status(
                "Preparing fake image runtime".to_owned(),
            )));
            let _ = tx.send(Ok(JobRuntimeEvent::Started));
            for step in 1..=total_steps {
                tokio::time::sleep(step_delay).await;
                if tx
                    .send(Ok(JobRuntimeEvent::Progress { step, total_steps }))
                    .is_err()
                {
                    // The consumer dropped the stream — a cancel. Mark cleanup.
                    if let Some(cleanup) = &cleanup {
                        cleanup.store(true, Ordering::SeqCst);
                    }
                    return;
                }
                if step == 1 {
                    let _ = tx.send(Ok(JobRuntimeEvent::Preview(vec![0x89, 0x50, 0x4E, 0x47])));
                }
            }
            let _ = tx.send(Ok(JobRuntimeEvent::Artifacts(vec![format!(
                "artifact-{model_id}"
            )])));
        });
        rx
    })
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition never became true");
}

async fn drain(mut rx: mpsc::UnboundedReceiver<JobEvent>) -> Vec<JobEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn a_job_streams_progress_and_preview_then_cancels_mid_run() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let cleanup = Arc::new(AtomicBool::new(false));
    let runner = image_runner_with_cleanup(
        "flux",
        4,
        Duration::from_millis(20),
        Some(Arc::clone(&cleanup)),
    );
    let id = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::String("a lighthouse".to_owned()),
        runner,
    );

    let mut rx = scheduler.events(&id);
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        if let JobEvent::Progress(progress) = &event
            && progress.step == Some(2)
        {
            scheduler.cancel(&id);
        }
        events.push(event);
    }

    assert_eq!(events.last(), Some(&JobEvent::Cancelled));
    assert!(events.contains(&JobEvent::Running));
    let running = events.iter().position(|e| *e == JobEvent::Running).unwrap();
    let first_progress = events
        .iter()
        .position(|e| matches!(e, JobEvent::Progress(_)))
        .unwrap();
    assert!(running < first_progress, "running precedes progress");

    let fractions: Vec<f64> = events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Progress(p) => Some(p.fraction),
            _ => None,
        })
        .collect();
    assert!(!fractions.is_empty());
    assert!(
        fractions.windows(2).all(|w| w[0] <= w[1]),
        "progress is monotone"
    );
    assert!(events.iter().any(|e| matches!(e, JobEvent::Preview(_))));

    wait_until(|| cleanup.load(Ordering::SeqCst)).await;

    let job = scheduler.job(&id).expect("job");
    assert_eq!(job.state, JobState::Cancelled);
    assert!(job.finished_at.is_some());
    assert!(job.result.is_empty());
}

#[tokio::test]
async fn a_job_runs_to_done_and_history_survives_a_reload() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let payload = JsonValue::Object(
        [(
            "prompt".to_owned(),
            JsonValue::String("a lighthouse".to_owned()),
        )]
        .into_iter()
        .collect(),
    );
    let id = scheduler.submit(
        "flux",
        Capability::image(),
        payload,
        image_runner("flux", 3, Duration::from_millis(5)),
    );

    let events = drain(scheduler.events(&id)).await;
    assert_eq!(
        events.last(),
        Some(&JobEvent::Done {
            result: vec!["artifact-flux".to_owned()]
        })
    );

    let live = scheduler.job(&id).expect("live job");
    assert_eq!(live.state, JobState::Done);
    assert_eq!(live.progress.fraction, 1.0);
    assert_eq!(live.result, vec!["artifact-flux".to_owned()]);

    let mut reloaded = JobHistoryStore::with_default_limit(dir.path());
    let recorded = reloaded.get(&id).expect("recorded");
    assert_eq!(recorded.state, JobState::Done);
    assert_eq!(recorded.result, vec!["artifact-flux".to_owned()]);
    assert_eq!(recorded.model_id, "flux");
    assert_eq!(recorded.capability, Capability::image());
    assert_eq!(
        recorded.payload.as_object().and_then(|o| o.get("prompt")),
        Some(&JsonValue::String("a lighthouse".to_owned()))
    );
    assert_eq!(recorded.error, None);
}

struct GateInner {
    open: AtomicBool,
    notify: Notify,
}

#[derive(Clone)]
struct GateAdmission {
    inner: Arc<GateInner>,
}

impl GateAdmission {
    fn new() -> Self {
        Self {
            inner: Arc::new(GateInner {
                open: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    fn release(&self) {
        self.inner.open.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }
}

impl JobAdmission for GateAdmission {
    fn admit(
        &self,
        _job: kernel::jobs::Job,
        on_wait: OnWait,
    ) -> BoxFuture<Result<RamVerdict, JobError>> {
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            if inner.open.load(Ordering::SeqCst) {
                return Ok(RamVerdict::Ok);
            }
            on_wait("Waiting for 30 GB of memory".to_owned()).await;
            loop {
                let notified = inner.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if inner.open.load(Ordering::SeqCst) {
                    return Ok(RamVerdict::Ok);
                }
                notified.await;
            }
        })
    }
}

#[tokio::test]
async fn a_job_waits_in_queued_visibly_until_admitted() {
    let dir = TempDir::new();
    let admission = GateAdmission::new();
    let scheduler = JobScheduler::new(
        JobHistoryStore::with_default_limit(dir.path()),
        Arc::new(admission.clone()),
        None,
    );
    let id = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 2, Duration::from_millis(5)),
    );

    let probe = scheduler.clone();
    let watched = id.clone();
    wait_until(move || probe.job(&watched).and_then(|j| j.queue_reason).is_some()).await;
    let waiting = scheduler.job(&id).expect("waiting job");
    assert_eq!(waiting.state, JobState::Queued);
    assert_eq!(
        waiting.queue_reason.as_deref(),
        Some("Waiting for 30 GB of memory")
    );

    let rx = scheduler.events(&id);
    admission.release();
    let events = drain(rx).await;

    assert_eq!(
        events.first(),
        Some(&JobEvent::Queued {
            reason: Some("Waiting for 30 GB of memory".to_owned())
        })
    );
    let queued = events
        .iter()
        .position(|e| matches!(e, JobEvent::Queued { .. }))
        .unwrap();
    let running = events.iter().position(|e| *e == JobEvent::Running).unwrap();
    assert!(queued < running);
    assert!(matches!(events.last(), Some(JobEvent::Done { .. })));

    let finished = scheduler.job(&id).expect("finished");
    assert_eq!(finished.state, JobState::Done);
    assert_eq!(finished.queue_reason, None);
}

#[tokio::test]
async fn cancelling_a_job_waiting_for_admission_unblocks_the_queue() {
    let dir = TempDir::new();
    let admission = GateAdmission::new();
    let scheduler = JobScheduler::new(
        JobHistoryStore::with_default_limit(dir.path()),
        Arc::new(admission.clone()),
        None,
    );
    let first = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 2, Duration::from_millis(5)),
    );
    let second = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 2, Duration::from_millis(5)),
    );

    let (s, f) = (scheduler.clone(), first.clone());
    wait_until(move || s.job(&f).and_then(|j| j.queue_reason).is_some()).await;
    scheduler.cancel(&first);

    let (s, f) = (scheduler.clone(), first.clone());
    wait_until(move || s.job(&f).map(|j| j.state) == Some(JobState::Cancelled)).await;
    assert_eq!(scheduler.job(&first).and_then(|j| j.started_at), None);

    let (s, sec) = (scheduler.clone(), second.clone());
    wait_until(move || s.job(&sec).and_then(|j| j.queue_reason).is_some()).await;
    admission.release();
    drain(scheduler.events(&second)).await;
    assert_eq!(
        scheduler.job(&second).map(|j| j.state),
        Some(JobState::Done)
    );
}

#[tokio::test]
async fn execution_is_serial_one_job_at_a_time() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let current = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));

    let probing_runner = |current: Arc<AtomicU32>, peak: Arc<AtomicU32>| -> Runner {
        Box::new(move || {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                let _ = tx.send(Ok(JobRuntimeEvent::Started));
                tokio::time::sleep(Duration::from_millis(40)).await;
                current.fetch_sub(1, Ordering::SeqCst);
                let _ = tx.send(Ok(JobRuntimeEvent::Artifacts(vec!["artifact".to_owned()])));
            });
            rx
        })
    };

    let first = scheduler.submit(
        "a",
        Capability::image(),
        JsonValue::Null,
        probing_runner(Arc::clone(&current), Arc::clone(&peak)),
    );
    let second = scheduler.submit(
        "b",
        Capability::image(),
        JsonValue::Null,
        probing_runner(Arc::clone(&current), Arc::clone(&peak)),
    );

    let (s, f) = (scheduler.clone(), first.clone());
    wait_until(move || s.job(&f).map(|j| j.state) == Some(JobState::Running)).await;
    assert_eq!(
        scheduler.job(&second).map(|j| j.state),
        Some(JobState::Queued)
    );

    drain(scheduler.events(&second)).await;
    assert_eq!(scheduler.job(&first).map(|j| j.state), Some(JobState::Done));
    assert_eq!(
        scheduler.job(&second).map(|j| j.state),
        Some(JobState::Done)
    );
    assert_eq!(peak.load(Ordering::SeqCst), 1, "never two at once");
}

#[tokio::test]
async fn cancelling_a_queued_job_removes_it_without_running() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let first = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 20, Duration::from_millis(20)),
    );
    let second = scheduler.submit(
        "flux",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 20, Duration::from_millis(20)),
    );

    let (s, f) = (scheduler.clone(), first.clone());
    wait_until(move || s.job(&f).map(|j| j.state) == Some(JobState::Running)).await;
    scheduler.cancel(&second);

    let cancelled = scheduler.job(&second).expect("second");
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert_eq!(cancelled.started_at, None);

    let mut history = JobHistoryStore::with_default_limit(dir.path());
    assert_eq!(
        history.get(&second).map(|j| j.state),
        Some(JobState::Cancelled)
    );

    scheduler.cancel(&first);
    drain(scheduler.events(&first)).await;
    assert_eq!(
        scheduler.job(&first).map(|j| j.state),
        Some(JobState::Cancelled)
    );
}

struct RecordingSink;

impl ArtifactWriting for RecordingSink {
    fn write(
        &self,
        _data: Vec<u8>,
        file_extension: String,
        job: kernel::jobs::Job,
    ) -> BoxFuture<Result<String, String>> {
        Box::pin(async move { Ok(format!("art-{}-{file_extension}", job.id)) })
    }
}

fn result_runner() -> Runner {
    Box::new(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let _ = tx.send(Ok(JobRuntimeEvent::Started));
            let _ = tx.send(Ok(JobRuntimeEvent::Result {
                data: vec![1, 2, 3],
                file_extension: "png".to_owned(),
            }));
        });
        rx
    })
}

#[tokio::test]
async fn result_events_persist_through_the_artifact_sink() {
    let dir = TempDir::new();
    let scheduler = JobScheduler::new(
        JobHistoryStore::with_default_limit(dir.path()),
        Arc::new(runtime::jobs::ImmediateAdmission),
        Some(Arc::new(RecordingSink)),
    );
    let id = scheduler.submit("m", Capability::image(), JsonValue::Null, result_runner());

    drain(scheduler.events(&id)).await;
    let job = scheduler.job(&id).expect("job");
    assert_eq!(job.state, JobState::Done);
    assert_eq!(job.result, vec![format!("art-{id}-png")]);
}

fn panicking_runner() -> Runner {
    // Panics when the scheduler calls it to build the stream — i.e. inside the
    // execute task, which is exactly the unwind the panic guard must catch.
    Box::new(|| panic!("runner construction boom"))
}

#[tokio::test]
async fn a_panicking_runner_fails_its_job_and_does_not_wedge_the_queue() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let first = scheduler.submit(
        "m",
        Capability::image(),
        JsonValue::Null,
        panicking_runner(),
    );
    let second = scheduler.submit(
        "m",
        Capability::image(),
        JsonValue::Null,
        image_runner("flux", 2, Duration::from_millis(5)),
    );

    let (s, f) = (scheduler.clone(), first.clone());
    wait_until(move || s.job(&f).map(|j| j.state) == Some(JobState::Failed)).await;

    let events = drain(scheduler.events(&second)).await;
    assert!(
        matches!(events.last(), Some(JobEvent::Done { .. })),
        "the queue advances past the panicked job"
    );
    assert_eq!(
        scheduler.job(&second).map(|j| j.state),
        Some(JobState::Done)
    );
}

#[tokio::test]
async fn a_result_with_no_sink_fails_the_job() {
    let dir = TempDir::new();
    let scheduler = scheduler(&dir);
    let id = scheduler.submit("m", Capability::image(), JsonValue::Null, result_runner());

    let events = drain(scheduler.events(&id)).await;
    assert!(
        matches!(events.last(), Some(JobEvent::Failed { .. })),
        "missing sink fails the job"
    );
    assert_eq!(scheduler.job(&id).map(|j| j.state), Some(JobState::Failed));
}
