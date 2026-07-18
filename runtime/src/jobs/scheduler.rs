//! The single-at-a-time job scheduler.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use kernel::jobs::{Job, JobEvent, JobHistoryStore, JobProgress, JobRuntimeEvent, JobState};
use kernel::records::{Capability, JsonValue};
use tokio::sync::{Notify, mpsc};

use crate::governor::{RamVerdict, lock};

use super::{ArtifactWriting, ImmediateAdmission, JobAdmission, JobError, OnWait, Runner};

const MISSING_SINK: &str = "No artifact store is attached to the job scheduler.";
const PANIC_MESSAGE: &str = "the job runner panicked";

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Executing {
    job_id: String,
    cancel: Arc<Notify>,
}

struct State {
    jobs: HashMap<String, Job>,
    queue: VecDeque<String>,
    runners: HashMap<String, Runner>,
    subscribers: HashMap<String, Vec<mpsc::UnboundedSender<JobEvent>>>,
    executing: Option<Executing>,
    cancel_requested: HashSet<String>,
}

struct Inner {
    state: Mutex<State>,
    history: Mutex<JobHistoryStore>,
    admission: Arc<dyn JobAdmission>,
    artifacts: Option<Arc<dyn ArtifactWriting>>,
}

/// Runs jobs one at a time in submission order. Cheap to clone (an `Arc` handle).
#[derive(Clone)]
pub struct JobScheduler {
    inner: Arc<Inner>,
}

impl JobScheduler {
    /// A scheduler recording into `history`, using `admission` and an optional
    /// artifact `sink`.
    pub fn new(
        history: JobHistoryStore,
        admission: Arc<dyn JobAdmission>,
        artifacts: Option<Arc<dyn ArtifactWriting>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    jobs: HashMap::new(),
                    queue: VecDeque::new(),
                    runners: HashMap::new(),
                    subscribers: HashMap::new(),
                    executing: None,
                    cancel_requested: HashSet::new(),
                }),
                history: Mutex::new(history),
                admission,
                artifacts,
            }),
        }
    }

    /// A scheduler with immediate admission and no artifact sink.
    pub fn with_history(history: JobHistoryStore) -> Self {
        Self::new(history, Arc::new(ImmediateAdmission), None)
    }

    /// Queue a job built from `runner`, returning its id. It starts as soon as it
    /// reaches the front of the (single) queue.
    pub fn submit(
        &self,
        model_id: &str,
        capability: Capability,
        payload: JsonValue,
        runner: Runner,
    ) -> String {
        let id = new_job_id();
        let job = Job::new(id.clone(), model_id, capability, payload, now_millis());
        let mut state = lock(&self.inner.state);
        state.jobs.insert(id.clone(), job);
        state.runners.insert(id.clone(), runner);
        state.queue.push_back(id.clone());
        self.start_next_if_idle(&mut state);
        id
    }

    /// The live job, or the recorded terminal one from history.
    pub fn job(&self, id: &str) -> Option<Job> {
        if let Some(live) = lock(&self.inner.state).jobs.get(id).cloned() {
            return Some(live);
        }
        lock(&self.inner.history).get(id)
    }

    /// The non-terminal jobs still in memory, ordered by `(submitted_at, id)`.
    pub fn active(&self) -> Vec<Job> {
        let state = lock(&self.inner.state);
        let mut jobs: Vec<Job> = state
            .jobs
            .values()
            .filter(|job| !job.state.is_terminal())
            .cloned()
            .collect();
        jobs.sort_by(|a, b| (a.submitted_at, &a.id).cmp(&(b.submitted_at, &b.id)));
        jobs
    }

    /// The queue length plus one when a job is executing.
    pub fn queue_depth(&self) -> usize {
        let state = lock(&self.inner.state);
        state.queue.len() + usize::from(state.executing.is_some())
    }

    /// A stream of a job's events: a coarse replay snapshot, then (for a live,
    /// non-terminal job) the future events until it concludes. A job no longer
    /// in memory replays its terminal snapshot from history.
    pub fn events(&self, id: &str) -> mpsc::UnboundedReceiver<JobEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut state = lock(&self.inner.state);
            if let Some(job) = state.jobs.get(id).cloned() {
                for event in replay(&job) {
                    let _ = tx.send(event);
                }
                if !job.state.is_terminal() {
                    state.subscribers.entry(id.to_owned()).or_default().push(tx);
                }
                return rx;
            }
        }
        if let Some(job) = lock(&self.inner.history).get(id)
            && job.state.is_terminal()
        {
            for event in replay(&job) {
                let _ = tx.send(event);
            }
        }
        rx
    }

    /// Cancel a job. A queued job is removed and concluded at once; the executing
    /// job is asked to stop cooperatively at its next event boundary.
    pub fn cancel(&self, job_id: &str) {
        enum Action {
            Notify(Arc<Notify>),
            ConcludeQueued,
        }
        let action = {
            let mut state = lock(&self.inner.state);
            match state.jobs.get(job_id) {
                Some(job) if !job.state.is_terminal() => {}
                _ => return,
            }
            match state.executing.as_ref() {
                Some(executing) if executing.job_id == job_id => {
                    let cancel = Arc::clone(&executing.cancel);
                    state.cancel_requested.insert(job_id.to_owned());
                    Action::Notify(cancel)
                }
                _ => {
                    state.queue.retain(|queued| queued != job_id);
                    Action::ConcludeQueued
                }
            }
        };
        match action {
            Action::Notify(cancel) => cancel.notify_one(),
            Action::ConcludeQueued => self.conclude(job_id, JobState::Cancelled, None),
        }
    }

    fn start_next_if_idle(&self, state: &mut State) {
        if state.executing.is_some() {
            return;
        }
        let Some(job_id) = state.queue.pop_front() else {
            return;
        };
        let cancel = Arc::new(Notify::new());
        state.executing = Some(Executing {
            job_id: job_id.clone(),
            cancel: Arc::clone(&cancel),
        });
        let scheduler = self.clone();
        tokio::spawn(async move {
            scheduler.execute(job_id, cancel).await;
        });
    }

    async fn execute(&self, job_id: String, cancel: Arc<Notify>) {
        // If `drive` panics (in caller-provided runner/admission/sink code) the
        // guard concludes the job so the `executing` slot is freed and the queue
        // is not silently wedged forever.
        let guard = ExecuteGuard::new(self.clone(), job_id.clone());
        let (terminal, error) = self.drive(&job_id, cancel).await;
        guard.disarm();
        self.conclude(&job_id, terminal, error);
    }

    async fn drive(&self, job_id: &str, cancel: Arc<Notify>) -> (JobState, Option<String>) {
        let (job, runner) = {
            let mut state = lock(&self.inner.state);
            match (
                state.jobs.get(job_id).cloned(),
                state.runners.remove(job_id),
            ) {
                (Some(job), Some(runner)) => (job, runner),
                _ => {
                    return (
                        JobState::Failed,
                        Some(format!("job {job_id} lost its runner")),
                    );
                }
            }
        };

        let on_wait: OnWait = {
            let scheduler = self.clone();
            let id = job_id.to_owned();
            Arc::new(move |reason: String| {
                let scheduler = scheduler.clone();
                let id = id.clone();
                Box::pin(async move { scheduler.mark_waiting(&id, &reason) }) as _
            })
        };
        let verdict = tokio::select! {
            verdict = self.inner.admission.admit(job, on_wait) => verdict,
            _ = cancel.notified() => return (JobState::Cancelled, None),
        };
        let verdict = match verdict {
            Ok(verdict) => verdict,
            Err(JobError::Cancelled) => return (JobState::Cancelled, None),
            Err(JobError::Failed(message)) => return (JobState::Failed, Some(message)),
        };
        if self.is_cancel_requested(job_id) {
            return (JobState::Cancelled, None);
        }

        self.mutate(job_id, |job| {
            job.state = JobState::Preparing;
            job.queue_reason = None;
            job.started_at = Some(now_millis());
        });
        self.emit(job_id, JobEvent::Preparing);
        if verdict == RamVerdict::Tight {
            self.emit(
                job_id,
                JobEvent::Status("Memory is tight — running anyway".to_owned()),
            );
        }

        let mut stream = runner();
        loop {
            if self.is_cancel_requested(job_id) {
                return (JobState::Cancelled, None);
            }
            let event = tokio::select! {
                event = stream.recv() => event,
                _ = cancel.notified() => return (JobState::Cancelled, None),
            };
            match event {
                None => {
                    let terminal = if self.is_cancel_requested(job_id) {
                        JobState::Cancelled
                    } else {
                        JobState::Done
                    };
                    return (terminal, None);
                }
                Some(Ok(JobRuntimeEvent::Result {
                    data,
                    file_extension,
                })) => {
                    if let Err(message) = self.persist(data, file_extension, job_id).await {
                        return (JobState::Failed, Some(message));
                    }
                }
                Some(Ok(event)) => self.apply(event, job_id),
                Some(Err(JobError::Cancelled)) => return (JobState::Cancelled, None),
                Some(Err(JobError::Failed(message))) => return (JobState::Failed, Some(message)),
            }
        }
    }

    fn apply(&self, event: JobRuntimeEvent, job_id: &str) {
        match event {
            JobRuntimeEvent::Status(message) => self.emit(job_id, JobEvent::Status(message)),
            JobRuntimeEvent::Started => self.mark_running(job_id),
            JobRuntimeEvent::Progress { step, total_steps } => {
                self.mark_running(job_id);
                let mut state = lock(&self.inner.state);
                let Some(job) = state.jobs.get_mut(job_id) else {
                    return;
                };
                let fraction = if total_steps > 0 {
                    (step as f64 / total_steps as f64).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                if fraction < job.progress.fraction {
                    return;
                }
                let progress = JobProgress::new(fraction, Some(step), Some(total_steps));
                job.progress = progress;
                emit_into(&mut state, job_id, JobEvent::Progress(progress));
            }
            JobRuntimeEvent::Preview(frame) => {
                let mut state = lock(&self.inner.state);
                if let Some(job) = state.jobs.get_mut(job_id) {
                    job.preview = Some(frame.clone());
                }
                emit_into(&mut state, job_id, JobEvent::Preview(frame));
            }
            JobRuntimeEvent::Artifacts(ids) => {
                self.mutate(job_id, |job| job.result.extend(ids));
            }
            JobRuntimeEvent::Result { .. } => {}
        }
    }

    async fn persist(
        &self,
        data: Vec<u8>,
        file_extension: String,
        job_id: &str,
    ) -> Result<(), String> {
        let job = {
            let state = lock(&self.inner.state);
            state.jobs.get(job_id).cloned()
        };
        let Some(job) = job else {
            return Ok(());
        };
        let Some(artifacts) = self.inner.artifacts.clone() else {
            return Err(MISSING_SINK.to_owned());
        };
        let artifact_id = artifacts.write(data, file_extension, job).await?;
        self.mutate(job_id, |job| job.result.push(artifact_id));
        Ok(())
    }

    fn mark_running(&self, job_id: &str) {
        let mut state = lock(&self.inner.state);
        match state.jobs.get_mut(job_id) {
            Some(job) if job.state != JobState::Running => job.state = JobState::Running,
            _ => return,
        }
        emit_into(&mut state, job_id, JobEvent::Running);
    }

    fn mark_waiting(&self, job_id: &str, reason: &str) {
        let mut state = lock(&self.inner.state);
        match state.jobs.get_mut(job_id) {
            Some(job) if job.state == JobState::Queued => {
                job.queue_reason = Some(reason.to_owned())
            }
            _ => return,
        }
        emit_into(
            &mut state,
            job_id,
            JobEvent::Queued {
                reason: Some(reason.to_owned()),
            },
        );
    }

    fn conclude(&self, job_id: &str, terminal: JobState, error: Option<String>) {
        let recorded = {
            let mut state = lock(&self.inner.state);
            let stamped = match state.jobs.get_mut(job_id) {
                Some(job) if !job.state.is_terminal() => {
                    job.state = terminal;
                    job.error = error.clone();
                    job.finished_at = Some(now_millis());
                    if terminal == JobState::Done {
                        let total = job.progress.total_steps;
                        job.progress = JobProgress::new(1.0, total.or(job.progress.step), total);
                    }
                    true
                }
                _ => false,
            };
            if !stamped {
                self.evict_and_advance(&mut state, job_id);
                return;
            }
            state.jobs.get(job_id).cloned()
        };
        let Some(recorded) = recorded else {
            return;
        };

        let event = match terminal {
            JobState::Done => JobEvent::Done {
                result: recorded.result.clone(),
            },
            JobState::Failed => JobEvent::Failed {
                message: error.unwrap_or_else(|| "failed".to_owned()),
            },
            JobState::Cancelled => JobEvent::Cancelled,
            _ => {
                self.evict_and_advance(&mut lock(&self.inner.state), job_id);
                return;
            }
        };

        // Record off the state lock so a job's disk write does not stall every
        // other scheduler operation (and the next job's start). The job stays in
        // memory (already terminal) until after the record, so a concurrent
        // observer still sees it.
        {
            let mut history = lock(&self.inner.history);
            let _ = history.record(recorded);
        }

        let mut state = lock(&self.inner.state);
        emit_into(&mut state, job_id, event);
        self.evict_and_advance(&mut state, job_id);
    }

    fn evict_and_advance(&self, state: &mut State, job_id: &str) {
        state.subscribers.remove(job_id);
        state.jobs.remove(job_id);
        state.runners.remove(job_id);
        state.cancel_requested.remove(job_id);
        if state
            .executing
            .as_ref()
            .is_some_and(|executing| executing.job_id == job_id)
        {
            state.executing = None;
        }
        self.start_next_if_idle(state);
    }

    fn mutate(&self, job_id: &str, change: impl FnOnce(&mut Job)) {
        let mut state = lock(&self.inner.state);
        if let Some(job) = state.jobs.get_mut(job_id) {
            change(job);
        }
    }

    fn emit(&self, job_id: &str, event: JobEvent) {
        let mut state = lock(&self.inner.state);
        emit_into(&mut state, job_id, event);
    }

    fn is_cancel_requested(&self, job_id: &str) -> bool {
        lock(&self.inner.state).cancel_requested.contains(job_id)
    }
}

struct ExecuteGuard {
    scheduler: JobScheduler,
    job_id: String,
    armed: bool,
}

impl ExecuteGuard {
    fn new(scheduler: JobScheduler, job_id: String) -> Self {
        Self {
            scheduler,
            job_id,
            armed: true,
        }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ExecuteGuard {
    fn drop(&mut self) {
        if self.armed {
            self.scheduler.conclude(
                &self.job_id,
                JobState::Failed,
                Some(PANIC_MESSAGE.to_owned()),
            );
        }
    }
}

fn emit_into(state: &mut State, job_id: &str, event: JobEvent) {
    if let Some(subscribers) = state.subscribers.get_mut(job_id) {
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }
}

fn replay(job: &Job) -> Vec<JobEvent> {
    let mut events = Vec::new();
    match job.state {
        JobState::Queued => events.push(JobEvent::Queued {
            reason: job.queue_reason.clone(),
        }),
        JobState::Preparing => events.push(JobEvent::Preparing),
        JobState::Running => {
            events.push(JobEvent::Running);
            if job.progress.fraction > 0.0 {
                events.push(JobEvent::Progress(job.progress));
            }
        }
        JobState::Done => events.push(JobEvent::Done {
            result: job.result.clone(),
        }),
        JobState::Failed => events.push(JobEvent::Failed {
            message: job.error.clone().unwrap_or_else(|| "failed".to_owned()),
        }),
        JobState::Cancelled => events.push(JobEvent::Cancelled),
    }
    if let Some(preview) = &job.preview
        && !job.state.is_terminal()
    {
        events.push(JobEvent::Preview(preview.clone()));
    }
    events
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn new_job_id() -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("job-{:016x}-{counter:x}", entropy())
}

fn entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}
