//! The process that owns one pull.
//!
//! A pull runs in a worker of its own so the download survives the terminal or
//! the TUI that asked for it. The worker holds the job's lock for its whole
//! life, takes one of a fixed number of slots while it transfers, writes what it
//! is doing into the job's record, honours the control file a client writes, and
//! retries a transfer that failed on something a retry can fix.
//!
//! Everything it coordinates through is a file, so a client that reads the job
//! directory sees the same truth whether it started the pull or not. That rests
//! on Unix advisory locks: the slots, the per-reference claim, and the job's own
//! lock all mean "held by a live process" only where `flock` does.
//!
//! Two deliberate costs. The record goes through the kernel's atomic write,
//! which fsyncs, so it is written at most twice a second rather than on every
//! progress event. And a stop reaches a provider as a dropped receiver, so the
//! difference between a pause and a cancel is carried separately, through
//! [`InstallService::stop_keeping`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kernel::install::plan::InstallPlan;
use kernel::install::pulls::{
    self, PullControl, PullError, PullEventKind, PullJob, PullJobDir, PullLock, PullState,
    PullStore,
};
use kernel::install::{InstallError, InstallEvent, InstallProgress};
use kernel::time::now_millis;
use sha2::{Digest, Sha256};

use crate::governor::BoxFuture;
use crate::settings::PullSettings;

use super::service::{InstallEventFeed, InstallService};

/// How many times a worker retries its own lock before deciding the job belongs
/// to someone else. A reader probing liveness holds a shared lock for an
/// instant, and that is enough to deny one exclusive claim.
const CLAIM_ATTEMPTS: u32 = 5;
/// The wait between those attempts.
const CLAIM_RETRY: Duration = Duration::from_millis(50);
/// How often the control file is read while the worker is busy.
const CONTROL_POLL: Duration = Duration::from_millis(250);
/// How often a queued worker looks for a free slot.
const SLOT_POLL: Duration = Duration::from_secs(2);
/// The shortest gap between two writes of the live record. Progress arrives far
/// faster than a reader can use, and each write is a full atomic rewrite.
const STATUS_INTERVAL: Duration = Duration::from_millis(500);
/// Where the concurrency slots live, under the pull store's root.
const SLOTS_DIR: &str = "slots";
/// Where the per-reference claims live, under the pull store's root.
const LOCKS_DIR: &str = "locks";
/// How much of the reference's hash names its claim file. Sixty-four bits is
/// past any chance of two references colliding on one machine.
const CLAIM_NAME_CHARS: usize = 16;

/// A failure running a pull worker.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// Another worker already holds this job.
    #[error("this pull is already running")]
    AlreadyRunning,

    /// Another worker is already pulling this reference.
    #[error("{0} is already being pulled")]
    AlreadyPulling(String),

    /// The job has ended, so there is nothing left to run.
    #[error("already {0}")]
    Ended(PullState),

    /// A cancel is on its way to the job and has not been read yet.
    #[error("a cancel is already on its way")]
    Cancelling,

    /// The job's record could not be read or written.
    #[error(transparent)]
    Record(#[from] PullError),

    /// A filesystem operation failed.
    #[error("pull worker io error: {0}")]
    Io(#[from] io::Error),
}

/// When a failed transfer is worth trying again, and how long to wait.
///
/// The schedule is per streak, not per job: an attempt that moved new bytes
/// proves the link works, so it clears the streak and the next failure starts
/// again at the shortest wait. The window is what ends a job that is getting
/// nowhere. This replaces the "a connect failure does not burn an attempt" rule
/// the plan sketched, which needed the transport to say why it failed; measuring
/// bytes says the same thing without reading error strings.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    steps: Vec<Duration>,
    window_ms: i64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(default_steps(), Duration::from_secs(2 * 60 * 60))
    }
}

/// The waits between attempts, shortest first.
fn default_steps() -> Vec<Duration> {
    vec![
        Duration::from_secs(5),
        Duration::from_secs(15),
        Duration::from_secs(45),
        Duration::from_secs(120),
        Duration::from_secs(300),
    ]
}

impl RetryPolicy {
    /// A policy waiting `steps` between attempts and giving up after `window`.
    /// An empty ladder falls back to the default one, since a policy that waits
    /// no time at all would spin.
    pub fn new(steps: Vec<Duration>, window: Duration) -> Self {
        Self {
            steps: match steps.is_empty() {
                true => default_steps(),
                false => steps,
            },
            window_ms: window.as_millis().min(i64::MAX as u128) as i64,
        }
    }

    /// The policy the settings describe, keeping the default waits.
    pub fn from_settings(settings: &PullSettings) -> Self {
        Self::new(default_steps(), settings.retry_window())
    }

    /// This policy giving up after `window` instead.
    pub fn with_window(mut self, window: Duration) -> Self {
        self.window_ms = window.as_millis().min(i64::MAX as u128) as i64;
        self
    }

    /// How long to wait before attempt number `attempt` of a streak (the first
    /// failure is attempt one). Waits hold at the last step rather than growing
    /// forever.
    pub fn delay(&self, attempt: u32) -> Duration {
        let index = (attempt.max(1) as usize - 1).min(self.steps.len().saturating_sub(1));
        self.steps.get(index).copied().unwrap_or_default()
    }

    /// Whether a streak that began at `began_ms` has run out of window by
    /// `now_ms`.
    pub fn spent(&self, began_ms: i64, now_ms: i64) -> bool {
        now_ms.saturating_sub(began_ms) >= self.window_ms
    }

    /// Whether `error` is worth another attempt.
    ///
    /// Everything that is a fact about the model or the machine is final: a repo
    /// or file that is not there, one that needs a token, a file that failed its
    /// checksum, a disk without room. What is left is the network, and the
    /// network comes back.
    pub fn retryable(error: &InstallError) -> bool {
        match error {
            InstallError::TransferFailed(_) | InstallError::ProviderUnavailable(_) => true,
            InstallError::ProviderUnknown(_)
            | InstallError::ReferenceInvalid(_)
            | InstallError::ReferenceNotFound(_)
            | InstallError::AuthRequired(_)
            | InstallError::InsufficientDisk { .. }
            | InstallError::ChecksumMismatch(_) => false,
        }
    }
}

/// The fixed set of slots that caps how many pulls transfer at once.
///
/// A slot is a locked file rather than a counter, so the cap holds across
/// processes and a worker that dies releases its slot without telling anyone.
#[derive(Debug, Clone)]
pub struct SlotPool {
    directory: PathBuf,
    slots: usize,
}

impl SlotPool {
    /// A pool of `slots` under the pull store rooted at `root`.
    pub fn new(root: impl AsRef<Path>, slots: usize) -> Self {
        Self {
            directory: root.as_ref().join(SLOTS_DIR),
            slots: slots.max(1),
        }
    }

    /// Take a free slot, or `None` when every one of them is busy.
    pub fn try_take(&self) -> Result<Option<PullLock>, WorkerError> {
        std::fs::create_dir_all(&self.directory)?;
        for index in 0..self.slots {
            let path = self.directory.join(format!("slot-{index}"));
            if let Some(lock) = pulls::take_lock(&path)? {
                return Ok(Some(lock));
            }
        }
        Ok(None)
    }
}

/// Claim `reference` on `provider` under the pull store rooted at `root`, or
/// `None` when another worker holds it. Two pulls of one model into one place
/// would fight over the same half-written files.
pub fn claim_reference(
    root: impl AsRef<Path>,
    provider: &str,
    reference: &str,
) -> Result<Option<PullLock>, WorkerError> {
    let directory = root.as_ref().join(LOCKS_DIR);
    std::fs::create_dir_all(&directory)?;
    let mut digest = Sha256::new();
    digest.update(provider.as_bytes());
    digest.update(b"|");
    digest.update(reference.as_bytes());
    let name = hex::encode(digest.finalize());
    Ok(pulls::take_lock(
        &directory.join(&name[..CLAIM_NAME_CHARS]),
    )?)
}

/// What a stop did.
///
/// The caller cannot work this out for itself. Reading the record before and
/// after would straddle a live worker, and a job that moved from queued to
/// running in between would read as though the stop had settled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The ask was left for the worker holding the job, which stops when it
    /// next reads the control file.
    Asked(PullState),
    /// Nobody was holding the job, so the record was settled here.
    Settled(PullState),
}

impl Stopped {
    /// The state the record now reads.
    pub fn state(self) -> PullState {
        match self {
            Self::Asked(state) | Self::Settled(state) => state,
        }
    }

    /// Whether the record was settled here rather than left for a worker.
    pub fn settled(self) -> bool {
        matches!(self, Self::Settled(_))
    }
}

/// Ask the job's worker to stop the way `control` says, and settle the record
/// here when no worker holds it, because there is nobody left to hear the ask.
///
/// The ask is written whatever the state, and left on disk once the record is
/// settled: a worker that was still starting reads it and stands down rather
/// than downloading over a job that is already over.
pub fn stop(job: &PullJobDir, control: PullControl) -> Result<Stopped, WorkerError> {
    let state = job.status().state;
    if state.is_terminal() {
        return Err(WorkerError::Ended(state));
    }
    job.request(control)?;
    // The record is read again after the liveness probe, never before it: a
    // worker finishing in the moment between the two would otherwise have its
    // `done` overwritten, for a model that is installed.
    if job.worker_alive() || job.stored_status().state.is_terminal() {
        return Ok(Stopped::Asked(job.status().state));
    }
    let settled = control.resulting_state();
    let now = now_millis();
    job.update_status(now, |status| {
        status.state = settled;
        status.next_attempt_at_ms = None;
        if settled.is_terminal() {
            status.pid = None;
        }
    })?;
    job.append(PullEventKind::State { state: settled }, now)?;
    Ok(Stopped::Settled(settled))
}

/// Put a worker back on a job that stopped, returning the new worker's pid.
///
/// The ask that stopped the job is dropped first: a worker spawned onto a job
/// whose control file still says `pause` would honour it and stop again before
/// moving a byte. A cancel is not dropped, because a cancel is not something to
/// undo on the way past; the job is refused instead.
///
/// The record is put back to queued before the spawn, and settled as failed if
/// the spawn does not happen, so a job is never left waiting for a process that
/// was never started.
#[cfg(unix)]
pub fn restart(job: &PullJobDir) -> Result<u32, WorkerError> {
    let state = job.status().state;
    if state.is_terminal() {
        return Err(WorkerError::Ended(state));
    }
    if job.worker_alive() {
        return Err(WorkerError::AlreadyRunning);
    }
    match job.control() {
        Some(PullControl::Cancel) => return Err(WorkerError::Cancelling),
        Some(control) => job.clear_control(control)?,
        None => {}
    }
    // The pid belongs to the worker that died; a queued record still holding one
    // reads as a job whose worker vanished rather than one waiting for its next.
    job.update_status(now_millis(), |status| {
        status.state = PullState::Queued;
        status.pid = None;
        status.message = None;
        status.next_attempt_at_ms = None;
    })?;
    spawn_worker(job)
}

/// What starting a pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// A job was created and a worker spawned on it; this is the job.
    Created(String),
    /// A pull of this model was already going, so this is that one.
    Joined,
    /// A pull of this model had stopped, and a worker was put back on it.
    Resumed,
}

/// Start a pull of `plan`, or join the pull of that model already under way.
///
/// Two pulls of one model would fight over the same half-written files, so a
/// job still going is joined rather than duplicated, and one that stopped with
/// bytes worth keeping is carried on from where it got to.
///
/// Asking for a model is an instruction, so a paused pull of it is resumed
/// here whatever `pull.auto_resume` says: that setting governs what happens
/// with nobody asking, which is [`resume_all`].
#[cfg(unix)]
pub fn start_or_join(store: &PullStore, plan: &InstallPlan) -> Result<Started, WorkerError> {
    if let Some(job) = store.under_way(&plan.provider, &plan.reference, now_millis()) {
        if !job.status().state.is_resumable() {
            return Ok(Started::Joined);
        }
        restart(&job)?;
        return Ok(Started::Resumed);
    }
    let job = store.create(plan, now_millis())?;
    spawn_worker(&job)?;
    Ok(Started::Created(job.id().to_owned()))
}

/// Spawn a worker on `job`, settling the job if it cannot be spawned: a job
/// left queued for a process that was never started would wait for good.
#[cfg(unix)]
fn spawn_worker(job: &PullJobDir) -> Result<u32, WorkerError> {
    match spawn_detached(job) {
        Ok(pid) => Ok(pid),
        Err(error) => {
            let message = format!("could not start a worker: {error}");
            let now = now_millis();
            job.update_status(now, |status| {
                status.state = PullState::Failed;
                status.message = Some(message);
            })?;
            job.append(
                PullEventKind::State {
                    state: PullState::Failed,
                },
                now,
            )?;
            Err(error.into())
        }
    }
}

/// Drop the records of ended pulls beyond the newest `pull.keep_ended`,
/// reporting how many went. The housekeeping a front end does when it opens
/// the store, since nothing else runs on a schedule: the listing would
/// otherwise grow until someone ran `hedos pull clean`.
pub fn collect_ended(store: &PullStore, settings: &PullSettings) -> usize {
    store.sweep(settings.kept_ended(), now_millis())
}

/// Put a worker back on every pull that stopped without anyone choosing to stop
/// it, reporting what happened to each by job id.
///
/// A pull whose worker died while the machine was asleep is the common case,
/// and the alternative is a list of downloads to restart by hand. A pull the
/// user paused is left exactly where they left it: nobody asked for it here,
/// and un-pausing it behind their back would make a pause impossible to keep.
#[cfg(unix)]
pub fn resume_all(store: &PullStore) -> Vec<(String, Result<u32, WorkerError>)> {
    store
        .jobs()
        .unwrap_or_default()
        .into_iter()
        .filter(|job| job.status().state == PullState::Interrupted)
        .map(|job| (job.id().to_owned(), restart(&job)))
        .collect()
}

/// What a worker does once a pull has landed: register it, so the model reaches
/// the shelf with nobody attached to watch.
pub type Registrar = Arc<dyn Fn() -> BoxFuture<Result<(), String>> + Send + Sync>;

/// Runs one pull to its end.
pub struct PullWorker {
    install: InstallService,
    slots: SlotPool,
    root: PathBuf,
    policy: RetryPolicy,
    registrar: Option<Registrar>,
}

impl PullWorker {
    /// A worker installing through `install`, coordinating through the pull
    /// store at `root`, and running as many pulls at once as `settings` allows.
    pub fn new(install: InstallService, root: impl Into<PathBuf>, settings: &PullSettings) -> Self {
        let root = root.into();
        Self {
            install,
            slots: SlotPool::new(&root, settings.slots()),
            root,
            policy: RetryPolicy::from_settings(settings),
            registrar: None,
        }
    }

    /// This worker with a different retry policy.
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// This worker registering what it pulled through `registrar`.
    pub fn with_registrar(mut self, registrar: Registrar) -> Self {
        self.registrar = Some(registrar);
        self
    }

    /// Run `job` to its end, returning the state it settled in.
    ///
    /// The job's lock is held for the whole call and released on the way out,
    /// however that happens: a client that finds the lock free and the record
    /// still saying `running` knows this worker died.
    pub async fn run(&self, job: &PullJobDir) -> Result<PullState, WorkerError> {
        let _claim = self.claim(job).await?.ok_or(WorkerError::AlreadyRunning)?;
        let descriptor = job.job().clone();
        // The pid goes down the moment the job is held, before anything that
        // can stand this worker back down. A job queued without one means
        // nothing has taken it, which is what tells a client waiting for a
        // worker from waiting for one that will never come.
        job.update_status(now_millis(), |status| {
            status.state = PullState::Queued;
            status.pid = Some(std::process::id());
            status.message = None;
            status.next_attempt_at_ms = None;
        })?;
        job.append(
            PullEventKind::State {
                state: PullState::Queued,
            },
            now_millis(),
        )?;

        let held = claim_reference(
            &self.root,
            descriptor.provider.as_str(),
            &descriptor.reference,
        )?;
        let Some(_reference) = held else {
            // Settled rather than left queued: this job is redundant, and a
            // record that stays live keeps a client joining it instead of the
            // pull that owns the reference.
            let message = format!("{} is already being pulled", descriptor.reference);
            self.settle(job, PullState::Failed, Some(message))?;
            return Err(WorkerError::AlreadyPulling(descriptor.reference));
        };

        self.transfer(job, &descriptor).await
    }

    /// Take the job's lock, giving a passing reader's shared probe a few
    /// chances to get out of the way before concluding someone else owns it.
    async fn claim(&self, job: &PullJobDir) -> Result<Option<PullLock>, WorkerError> {
        for attempt in 0..CLAIM_ATTEMPTS {
            if let Some(lock) = job.claim()? {
                return Ok(Some(lock));
            }
            if attempt + 1 < CLAIM_ATTEMPTS {
                tokio::time::sleep(CLAIM_RETRY).await;
            }
        }
        Ok(None)
    }

    /// Attempt the transfer until it lands, is stopped, or stops being worth
    /// retrying.
    async fn transfer(
        &self,
        job: &PullJobDir,
        descriptor: &PullJob,
    ) -> Result<PullState, WorkerError> {
        let mut attempts: u32 = 0;
        let mut streak: u32 = 0;
        let mut streak_began: Option<i64> = None;
        loop {
            if let Some(control) = job.control() {
                return self.honour(job, control);
            }
            // The slot is taken per attempt and given up before any wait, so a
            // job sleeping out a backoff does not keep another one queued.
            let slot = match self.wait_for_slot(job).await? {
                Waited::Ready(slot) => slot,
                Waited::Stopped(control) => return self.honour(job, control),
            };
            attempts += 1;
            self.mark(job, PullState::Running, None)?;
            job.update_status(now_millis(), |status| {
                status.attempt = attempts;
                status.next_attempt_at_ms = None;
                status.message = None;
            })?;

            let outcome = self.attempt_once(job, descriptor).await?;
            drop(slot);

            let (message, error, moved) = match outcome {
                Outcome::Done { asked } => {
                    if let Some(control) = asked {
                        // The download landed before the ask could stop it;
                        // leaving the file would stop the next worker instead.
                        job.clear_control(control)?;
                    }
                    return self.finish(job).await;
                }
                Outcome::Stopped(control) => return self.honour(job, control),
                Outcome::Cancelled => return self.settle(job, PullState::Cancelled, None),
                Outcome::Failed {
                    asked: Some(control),
                    ..
                } => return self.honour(job, control),
                Outcome::Failed {
                    message,
                    error,
                    moved,
                    asked: None,
                } => (message, error, moved),
            };

            // An unrecognised failure is treated as worth retrying: the window
            // below bounds it, and the common unrecognised case is the network.
            if !error.as_ref().is_none_or(RetryPolicy::retryable) {
                return self.settle(job, PullState::Failed, Some(message));
            }
            if moved {
                streak = 0;
                streak_began = None;
            }
            streak += 1;
            let now = now_millis();
            let began = *streak_began.get_or_insert(now);
            if self.policy.spent(began, now) {
                return self.settle(job, PullState::Interrupted, Some(message));
            }

            let delay = self.policy.delay(streak);
            job.append(
                PullEventKind::Retry {
                    attempt: attempts,
                    reason: message.clone(),
                    delay_ms: delay.as_millis() as i64,
                },
                now,
            )?;
            self.mark(job, PullState::Queued, None)?;
            job.update_status(now, |status| {
                status.next_attempt_at_ms = Some(now.saturating_add(delay.as_millis() as i64));
                status.message = Some(message);
            })?;
            if let Some(control) = self.rest(job, delay).await {
                return self.honour(job, control);
            }
        }
    }

    async fn wait_for_slot(&self, job: &PullJobDir) -> Result<Waited, WorkerError> {
        loop {
            if let Some(control) = job.control() {
                return Ok(Waited::Stopped(control));
            }
            if let Some(slot) = self.slots.try_take()? {
                return Ok(Waited::Ready(slot));
            }
            self.mark(job, PullState::Queued, None)?;
            if let Some(control) = self.rest(job, SLOT_POLL).await {
                return Ok(Waited::Stopped(control));
            }
        }
    }

    /// One resolve-and-transfer attempt.
    ///
    /// Resolving is a live request that can take as long as the network makes
    /// it, so the control file is watched alongside it: a pull asked to stop
    /// should not have to wait out a hub that has gone quiet.
    async fn attempt_once(
        &self,
        job: &PullJobDir,
        descriptor: &PullJob,
    ) -> Result<Outcome, WorkerError> {
        let resolving = self
            .install
            .plan(&descriptor.provider, &descriptor.reference);
        let planned = tokio::select! {
            planned = resolving => planned,
            control = self.await_control(job) => return Ok(Outcome::Stopped(control)),
        };
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => return Ok(Outcome::failed(error)),
        };
        let id = match self.install.begin(plan) {
            Ok(id) => id,
            Err(error) => return Ok(Outcome::failed(error)),
        };
        let mut events = self.install.events(&id);
        let mut outcome = self.drive(job, &id, &mut events).await?;
        if let Outcome::Failed { error, .. } = &mut outcome {
            *error = self.install.failure(&id);
        }
        Ok(outcome)
    }

    /// Follow one install's events into the job's record until it ends, reading
    /// the control file between them.
    async fn drive(
        &self,
        job: &PullJobDir,
        install_id: &str,
        events: &mut InstallEventFeed,
    ) -> Result<Outcome, WorkerError> {
        let mut asked: Option<PullControl> = None;
        let mut latest: Option<InstallProgress> = None;
        let mut written_at = 0i64;
        // What the attempt started from. Bytes already on disk are reported
        // before the first new one arrives, so only bytes past this line count
        // as progress.
        let mut baseline: Option<i64> = None;
        let mut moved = false;
        let mut ticker = tokio::time::interval(CONTROL_POLL);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(InstallEvent::Progress(progress)) => {
                        let base = *baseline.get_or_insert(progress.bytes_downloaded);
                        moved |= progress.bytes_downloaded > base;
                        latest = Some(progress);
                        self.flush(job, &mut latest, &mut written_at, false)?;
                    }
                    Some(InstallEvent::Status(text)) => {
                        let now = now_millis();
                        job.append(PullEventKind::Status { text: text.clone() }, now)?;
                        job.update_status(now, |status| status.status_line = Some(text))?;
                    }
                    Some(InstallEvent::Done) => {
                        self.flush(job, &mut latest, &mut written_at, true)?;
                        return Ok(Outcome::Done { asked });
                    }
                    Some(InstallEvent::Failed { message }) => {
                        return Ok(Outcome::Failed { message, error: None, moved, asked });
                    }
                    Some(InstallEvent::Cancelled) => {
                        return Ok(match asked {
                            Some(control) => Outcome::Stopped(control),
                            None => Outcome::Cancelled,
                        });
                    }
                    Some(InstallEvent::Queued | InstallEvent::Preparing) => {}
                    // The feed closes without a terminal event only if the
                    // service dropped it, which is a failure like any other.
                    None => return Ok(Outcome::Failed {
                        message: "the download ended without saying how".to_owned(),
                        error: None,
                        moved,
                        asked,
                    }),
                },
                _ = ticker.tick() => {
                    if asked.is_none() && let Some(control) = job.control() {
                        asked = Some(control);
                        match control {
                            // A pause keeps what landed; a cancel lets the
                            // provider tidy the half-download away.
                            PullControl::Pause => self.install.stop_keeping(install_id),
                            PullControl::Cancel => self.install.cancel(install_id),
                        }
                    }
                    self.flush(job, &mut latest, &mut written_at, false)?;
                }
            }
        }
    }

    /// Write the pending progress into the record, at most one write per
    /// [`STATUS_INTERVAL`] unless `force`.
    fn flush(
        &self,
        job: &PullJobDir,
        latest: &mut Option<InstallProgress>,
        written_at: &mut i64,
        force: bool,
    ) -> Result<(), WorkerError> {
        let Some(progress) = latest.take() else {
            return Ok(());
        };
        let now = now_millis();
        if !force && now.saturating_sub(*written_at) < STATUS_INTERVAL.as_millis() as i64 {
            *latest = Some(progress);
            return Ok(());
        }
        job.update_status(now, |status| status.progress = progress)?;
        *written_at = now;
        Ok(())
    }

    /// Register what landed, then mark the job done. A registration that fails
    /// is said out loud on the job rather than turning a finished download into
    /// a failure: the weights are on disk either way.
    async fn finish(&self, job: &PullJobDir) -> Result<PullState, WorkerError> {
        let message = match &self.registrar {
            Some(registrar) => registrar().await.err(),
            None => None,
        };
        self.settle(job, PullState::Done, message)
    }

    /// Stop the way `control` asked, and take the control file with it.
    fn honour(&self, job: &PullJobDir, control: PullControl) -> Result<PullState, WorkerError> {
        let state = self.settle(job, control.resulting_state(), None)?;
        job.clear_control(control)?;
        Ok(state)
    }

    /// Move the job to `state`, recording it in the history only when it is a
    /// change, so a job that queues between attempts does not fill its history
    /// with the same word.
    fn mark(
        &self,
        job: &PullJobDir,
        state: PullState,
        message: Option<String>,
    ) -> Result<PullState, WorkerError> {
        match job.stored_status().state == state {
            true => Ok(state),
            false => self.settle(job, state, message),
        }
    }

    fn settle(
        &self,
        job: &PullJobDir,
        state: PullState,
        message: Option<String>,
    ) -> Result<PullState, WorkerError> {
        let now = now_millis();
        job.update_status(now, |status| {
            status.state = state;
            status.next_attempt_at_ms = None;
            // Only a reason worth keeping overwrites one: every attempt clears
            // the message when it starts, so nothing stale survives into `done`.
            if message.is_some() {
                status.message = message;
            }
            if state.is_terminal() {
                status.pid = None;
            }
        })?;
        job.append(PullEventKind::State { state }, now)?;
        Ok(state)
    }

    /// Wait `how_long`, cut short by a control arriving.
    async fn rest(&self, job: &PullJobDir, how_long: Duration) -> Option<PullControl> {
        let deadline = tokio::time::Instant::now() + how_long;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return None;
            }
            tokio::time::sleep(CONTROL_POLL.min(deadline - now)).await;
            if let Some(control) = job.control() {
                return Some(control);
            }
        }
    }

    /// Wait for a control to be asked for, however long that takes. Only ever
    /// used as the losing half of a `select!`.
    async fn await_control(&self, job: &PullJobDir) -> PullControl {
        loop {
            if let Some(control) = job.control() {
                return control;
            }
            tokio::time::sleep(CONTROL_POLL).await;
        }
    }
}

enum Waited {
    Ready(PullLock),
    Stopped(PullControl),
}

/// How one attempt ended.
enum Outcome {
    /// The model landed. `asked` is a control that arrived too late to stop it.
    Done { asked: Option<PullControl> },
    /// A control was honoured instead of the transfer finishing.
    Stopped(PullControl),
    /// The install was cancelled by something other than this job's control.
    Cancelled,
    /// The transfer failed.
    Failed {
        message: String,
        /// Why, when the service still remembers the typed error.
        error: Option<InstallError>,
        /// Whether new bytes crossed the network before it failed.
        moved: bool,
        /// A control that arrived while it was failing.
        asked: Option<PullControl>,
    },
}

impl Outcome {
    /// An attempt that failed before any transfer began.
    fn failed(error: InstallError) -> Self {
        Self::Failed {
            message: error.to_string(),
            error: Some(error),
            moved: false,
            asked: None,
        }
    }
}

/// Start a worker for `job` in a process of its own and return its pid.
///
/// The child leaves this process's process group, so a Ctrl-C meant for the
/// shell that spawned it does not reach the download, and its output goes
/// nowhere: what it has to say, it says in the job's record. It is reaped on a
/// task only so it leaves no zombie behind; the download does not depend on this
/// process staying alive, and outlives it by reparenting.
///
/// Call this from inside a tokio runtime.
#[cfg(unix)]
pub fn spawn_detached(job: &PullJobDir) -> io::Result<u32> {
    let executable = std::env::current_exe()?;
    let mut child = tokio::process::Command::new(executable)
        .arg("pull-worker")
        .arg("--job")
        .arg(job.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .kill_on_drop(false)
        .spawn()?;
    let pid = child.id().unwrap_or_default();
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(pid)
}

/// Keep a closing terminal from taking the worker with it.
///
/// Handling the signal is what ignores it: the default action for `SIGHUP` is to
/// end the process, and installing a handler that never acts replaces it. Call
/// this from inside a tokio runtime.
#[cfg(unix)]
pub fn ignore_hangup() {
    use tokio::signal::unix::{SignalKind, signal};

    if let Ok(mut hangups) = signal(SignalKind::hangup()) {
        tokio::spawn(async move { while hangups.recv().await.is_some() {} });
    }
}
