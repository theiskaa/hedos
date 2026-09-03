//! The record a pull keeps on disk, so a download outlives the process that
//! started it.
//!
//! One directory per pull under `<data>/pulls/<id>/`: the descriptor
//! (`job.json`, written once), the live record (`status.json`, rewritten as the
//! transfer moves), an append-only history (`events.jsonl`), the lock a worker
//! holds for as long as it runs, and the `control` file a client writes to ask
//! for a pause or a cancel.
//!
//! Nothing here starts, signals, or waits on a process: this module owns the
//! format and the rules, the runtime owns the worker.
//!
//! The liveness rule rests on Unix advisory locks (`flock`), where a lock
//! belongs to an open file description rather than to a process. Readers take a
//! shared lock to test, so they never exclude one another; a worker takes the
//! exclusive one.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::install::event::InstallProgress;
use crate::install::plan::InstallPlan;
use crate::install::provider::InstallProviderId;
use crate::persistence::{self, StoreError};

const JOB_FILE: &str = "job.json";
const STATUS_FILE: &str = "status.json";
const EVENTS_FILE: &str = "events.jsonl";
const LOCK_FILE: &str = "lock";
const CONTROL_FILE: &str = "control";

/// The longest reference slug a job id carries; the timestamp in front is what
/// makes the id unique, the slug is only there to make it readable.
const SLUG_LIMIT: usize = 40;
/// How long a job queued with no worker is given before it counts as abandoned.
/// A worker writes its pid as soon as it holds the job, which it does within
/// milliseconds of starting, so this is generous rather than tuned.
pub const START_GRACE_MS: i64 = 3_000;
/// How many suffixed ids `create` tries before giving up. Two pulls of the same
/// reference in the same millisecond is already unlikely; sixty-four is a
/// runaway guard, not a working limit.
const CREATE_ATTEMPTS: u32 = 64;

/// A failure reading or writing a pull's record.
#[derive(Debug, thiserror::Error)]
pub enum PullError {
    /// A filesystem operation failed.
    #[error("pull io error: {0}")]
    Io(#[from] io::Error),

    /// A store helper failed.
    #[error("pull store error: {0}")]
    Store(#[from] StoreError),

    /// A descriptor exists but this build cannot decode it. It is left exactly
    /// where it is: another process may own the pull it describes.
    #[error("unreadable pull descriptor at {path}: {source}")]
    Unreadable {
        /// The descriptor that would not decode.
        path: PathBuf,
        /// Why it would not decode.
        #[source]
        source: serde_json::Error,
    },

    /// No job matched the id, prefix, or reference given.
    #[error("no pull matches \"{0}\"")]
    NotFound(String),

    /// More than one job matched, so the caller has to be more specific.
    #[error("\"{query}\" matches {count} pulls")]
    Ambiguous {
        /// What the caller asked for.
        query: String,
        /// How many jobs it matched.
        count: usize,
    },

    /// The job directory could not be named uniquely.
    #[error("could not claim a directory for a pull of {0}")]
    Unclaimable(String),
}

/// Where a pull is in its life.
///
/// `Paused` and `Interrupted` both mean "stopped with bytes worth keeping"; they
/// differ in who stopped it, which is what the user needs to know. `Interrupted`
/// is never written by the process that died: it is what a reader concludes from
/// a record whose worker no longer holds the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullState {
    /// Waiting for a free slot.
    Queued,
    /// Transferring.
    Running,
    /// Stopped by the user, resumable.
    Paused,
    /// Installed.
    Done,
    /// Ended on something retrying will not fix.
    Failed,
    /// Stopped by the user for good.
    Cancelled,
    /// Stopped by something other than the user, resumable.
    Interrupted,
}

impl PullState {
    /// Whether the job has ended for good (`Done`/`Failed`/`Cancelled`).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// Whether a worker could pick the job up again.
    pub fn is_resumable(self) -> bool {
        matches!(self, Self::Paused | Self::Interrupted)
    }

    /// Whether a worker should be running for this job right now.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    /// The lowercase word this state is written and shown as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

impl std::fmt::Display for PullState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a client asked a running worker to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullControl {
    /// Stop, keep the partial, stay resumable.
    Pause,
    /// Stop for good.
    Cancel,
}

impl PullControl {
    /// The bare word the control file carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Cancel => "cancel",
        }
    }

    /// The control a control file's contents name, if any.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "pause" => Some(Self::Pause),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    /// The state a worker lands in after honouring this control.
    pub fn resulting_state(self) -> PullState {
        match self {
            Self::Pause => PullState::Paused,
            Self::Cancel => PullState::Cancelled,
        }
    }
}

impl std::fmt::Display for PullControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A pull's descriptor: what was asked for, written once when the job is created.
///
/// It carries only what a listing needs. The authoritative [`InstallPlan`] is
/// resolved again by the worker, because `remaining_bytes` is stale the moment a
/// partial download exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullJob {
    /// The job id, which is also its directory name.
    pub id: String,
    /// The install provider that will fetch it.
    pub provider: InstallProviderId,
    /// The reference being installed (repo or tag).
    pub reference: String,
    /// The name to show.
    pub display_name: String,
    /// Where the model will land.
    pub destination: String,
    /// The resolved revision, when the plan pinned one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub revision: Option<String>,
    /// The plan's total size at creation, for a listing that has not started yet.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub total_bytes: Option<i64>,
    /// When the job was created, epoch milliseconds.
    pub created_at_ms: i64,
}

impl PullJob {
    /// The descriptor for `plan`, as job `id` created at `created_at_ms`.
    pub(crate) fn from_plan(plan: &InstallPlan, id: impl Into<String>, created_at_ms: i64) -> Self {
        Self {
            id: id.into(),
            provider: plan.provider.clone(),
            reference: plan.reference.clone(),
            display_name: plan.display_name.clone(),
            destination: plan.destination.clone(),
            revision: plan.revision.clone(),
            total_bytes: plan.total_bytes,
            created_at_ms,
        }
    }
}

/// A pull's live record, rewritten as the transfer moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullStatus {
    /// Where the pull is.
    pub state: PullState,
    /// How much has transferred.
    #[serde(default)]
    pub progress: InstallProgress,
    /// The provider's last human-readable line.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status_line: Option<String>,
    /// Which attempt is running; `0` until the first transfer starts.
    #[serde(default)]
    pub attempt: u32,
    /// When the next retry is due, epoch milliseconds, while one is waiting.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_attempt_at_ms: Option<i64>,
    /// Why the job ended, or why it is waiting to retry.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message: Option<String>,
    /// The worker's process id, for display only. Liveness comes from the lock.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pid: Option<u32>,
    /// When this record was last written, epoch milliseconds.
    pub updated_at_ms: i64,
}

impl PullStatus {
    /// A fresh record for a job that has not started, stamped `now`.
    pub fn queued(now: i64) -> Self {
        Self {
            state: PullState::Queued,
            progress: InstallProgress::default(),
            status_line: None,
            attempt: 0,
            next_attempt_at_ms: None,
            message: None,
            pid: None,
            updated_at_ms: now,
        }
    }
}

/// One line of a job's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullEvent {
    /// When it happened, epoch milliseconds.
    pub at_ms: i64,
    /// What happened.
    #[serde(flatten)]
    pub kind: PullEventKind,
}

/// What a history line records. Progress is deliberately absent: it belongs in
/// the rewritten status, not in a file that only grows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum PullEventKind {
    /// The job moved to a new state.
    State {
        /// The state it moved to.
        state: PullState,
    },
    /// The provider said something worth keeping.
    Status {
        /// The line.
        text: String,
    },
    /// A transfer failed and another attempt is scheduled.
    Retry {
        /// Which attempt just failed.
        attempt: u32,
        /// Why it failed.
        reason: String,
        /// How long until the next one, milliseconds.
        delay_ms: i64,
    },
}

/// A worker's claim on a job, held for as long as the worker runs. Dropping it
/// releases the lock, and so does the process ending for any reason, which is
/// what makes a lost worker detectable.
#[derive(Debug)]
pub struct PullLock {
    file: File,
}

impl Drop for PullLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// The directory of pull jobs.
#[derive(Debug, Clone)]
pub struct PullStore {
    root: PathBuf,
}

impl PullStore {
    /// A store over `root`, which is created when the first job is.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory the jobs live in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create a job for `plan` at `now`, claiming its directory and writing the
    /// descriptor and a `queued` record.
    pub fn create(&self, plan: &InstallPlan, now: i64) -> Result<PullJobDir, PullError> {
        let base = format!("{now}-{}", reference_slug(&plan.reference));
        fs::create_dir_all(&self.root)?;
        for attempt in 0..CREATE_ATTEMPTS {
            let id = match attempt {
                0 => base.clone(),
                _ => format!("{base}-{}", attempt + 1),
            };
            let path = self.root.join(&id);
            // `create_dir` failing on an existing directory is what claims the id
            // against another process racing for the same millisecond.
            match fs::create_dir(&path) {
                Ok(()) => {
                    let job = PullJob::from_plan(plan, id, now);
                    persistence::write_json_atomic(&path.join(JOB_FILE), &job)?;
                    let handle = PullJobDir { path, job };
                    handle.write_status(&PullStatus::queued(now))?;
                    return Ok(handle);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(PullError::Unclaimable(plan.reference.clone()))
    }

    /// The job with exactly this id.
    pub fn open(&self, id: &str) -> Result<PullJobDir, PullError> {
        PullJobDir::open(self.root.join(id))
    }

    /// Every readable job, oldest first. A directory without a readable
    /// descriptor is not a job and is skipped; so is a store that cannot be
    /// read at all, which [`PullStore::jobs`] reports instead of hiding.
    pub fn list(&self) -> Vec<PullJobDir> {
        self.jobs().unwrap_or_default()
    }

    /// Every readable job, oldest first, reporting a store that could not be
    /// read (a missing store is simply empty).
    pub fn jobs(&self) -> Result<Vec<PullJobDir>, PullError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut jobs: Vec<PullJobDir> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| PullJobDir::open(entry.path()).ok())
            .collect();
        jobs.sort_by(|left, right| {
            left.job
                .created_at_ms
                .cmp(&right.job.created_at_ms)
                .then_with(|| left.job.id.cmp(&right.job.id))
        });
        Ok(jobs)
    }

    /// The one job `query` names: an exact id, then an unambiguous id prefix,
    /// then an exact reference (ignoring case).
    pub fn resolve(&self, query: &str) -> Result<PullJobDir, PullError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(PullError::NotFound(String::new()));
        }
        let jobs = self.jobs()?;
        if let Some(job) = jobs.iter().find(|job| job.job.id == query) {
            return Ok(job.clone());
        }
        let by_prefix = jobs.iter().filter(|job| job.job.id.starts_with(query));
        if let Some(job) = single(by_prefix, query)? {
            return Ok(job.clone());
        }
        let by_reference = jobs
            .iter()
            .filter(|job| job.job.reference.eq_ignore_ascii_case(query));
        match single(by_reference, query)? {
            Some(job) => Ok(job.clone()),
            None => Err(PullError::NotFound(query.to_owned())),
        }
    }

    /// The newest job pulling `reference` from `provider` that a client could
    /// join: one still going, or one that stopped with bytes worth resuming.
    ///
    /// The reference is matched the way [`PullStore::resolve`] matches one, so a
    /// tag the provider rewrote (`ls` into `ls:latest`) only joins the job it
    /// created once the caller passes the rewritten form. A job whose worker
    /// never arrived is past joining: nothing is coming for it, and preferring
    /// it because it is newest would hide the pull that is actually running.
    pub fn under_way(
        &self,
        provider: &InstallProviderId,
        reference: &str,
        now_ms: i64,
    ) -> Option<PullJobDir> {
        self.list()
            .into_iter()
            .rev()
            .filter(|job| {
                job.job().provider == *provider
                    && job.job().reference.eq_ignore_ascii_case(reference)
            })
            .find(|job| !job.status().state.is_terminal() && !job.abandoned(now_ms, START_GRACE_MS))
    }

    /// Remove ended jobs last touched before `before_ms`, keeping the newest
    /// `keep` of them however old they are. Returns how many were removed.
    ///
    /// A job whose worker still holds the lock is never removed, however its
    /// record reads: a worker writes `done` and then registers what it fetched,
    /// and deleting the directory under it would leave a partial one behind.
    pub fn sweep(&self, keep: usize, before_ms: i64) -> usize {
        let mut ended: Vec<(i64, PullJobDir)> = self
            .list()
            .into_iter()
            .filter(|job| !job.worker_alive())
            .filter_map(|job| {
                let status = job.stored_status();
                status
                    .state
                    .is_terminal()
                    .then_some((status.updated_at_ms, job))
            })
            .collect();
        ended.sort_by_key(|(touched_at, _)| std::cmp::Reverse(*touched_at));
        let mut removed = 0;
        for (touched_at, job) in ended.into_iter().skip(keep) {
            if touched_at < before_ms && job.remove().is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// The single job `found` yields: `None` when it yields nothing, an
/// [`PullError::Ambiguous`] when it yields several that are equally plausible.
///
/// A name several jobs answer to means the one still going. Pulling a model a
/// second time would otherwise make its own name ambiguous for good, since the
/// first job keeps that name for as long as its record is kept.
fn single<'a>(
    found: impl Iterator<Item = &'a PullJobDir>,
    query: &str,
) -> Result<Option<&'a PullJobDir>, PullError> {
    let matches: Vec<&PullJobDir> = found.collect();
    match matches.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        many => {
            let going: Vec<&PullJobDir> = many
                .iter()
                .copied()
                .filter(|job| !job.status().state.is_terminal())
                .collect();
            match going.as_slice() {
                [only] => Ok(Some(only)),
                _ => Err(PullError::Ambiguous {
                    query: query.to_owned(),
                    count: many.len(),
                }),
            }
        }
    }
}

/// One pull's directory: its descriptor, and the files around it.
#[derive(Debug, Clone)]
pub struct PullJobDir {
    path: PathBuf,
    job: PullJob,
}

impl PullJobDir {
    /// Open the job directory at `path`, reading its descriptor.
    ///
    /// An undecodable descriptor is reported, never moved aside: another
    /// process may still be pulling what it describes, and a reader that
    /// quarantines it would take the job away from every client at once.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PullError> {
        let path = path.into();
        let descriptor = path.join(JOB_FILE);
        let bytes = match fs::read(&descriptor) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(PullError::NotFound(name_of(&path)));
            }
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_slice(&bytes) {
            Ok(job) => Ok(Self { path, job }),
            Err(source) => Err(PullError::Unreadable {
                path: descriptor,
                source,
            }),
        }
    }

    /// The directory itself.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The job id.
    pub fn id(&self) -> &str {
        &self.job.id
    }

    /// The descriptor.
    pub fn job(&self) -> &PullJob {
        &self.job
    }

    /// The lock a worker holds for as long as it owns this job.
    pub fn lock_path(&self) -> PathBuf {
        self.path.join(LOCK_FILE)
    }

    /// Take the job for this process, or `None` when someone else holds it.
    ///
    /// A single refusal is not proof of another worker: a reader probing
    /// liveness holds a shared lock for a moment, and that is enough to deny an
    /// exclusive claim. A caller concluding "already owned" should try a few
    /// times over a short window first.
    pub fn claim(&self) -> Result<Option<PullLock>, PullError> {
        take_lock(&self.lock_path())
    }

    /// Whether a worker still owns this job.
    ///
    /// The test is the lock, not the pid: a pid can be reused, and the operating
    /// system releases an advisory lock even when the process is killed or
    /// panics. The probe takes a *shared* lock, which a worker's exclusive one
    /// still blocks, but which two readers can hold at once: probing with an
    /// exclusive lock would make concurrent readers report each other as the
    /// worker.
    ///
    /// A lock file that exists but cannot be opened counts as alive. Calling a
    /// live pull dead is the costlier mistake, since it invites a second worker
    /// onto the same download.
    pub fn worker_alive(&self) -> bool {
        let file = match File::open(self.lock_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        };
        match file.try_lock_shared() {
            Ok(()) => {
                let _ = file.unlock();
                false
            }
            Err(_) => true,
        }
    }

    /// The record as written, without the liveness rule applied.
    ///
    /// A missing record reads as `queued`, which is what a job whose worker
    /// never got started is. An undecodable one reads the same way rather than
    /// being quarantined: this file is a rewritten view of live state, not a
    /// store of truth, and a running worker replaces it within the second.
    pub fn stored_status(&self) -> PullStatus {
        fs::read(self.path.join(STATUS_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| PullStatus::queued(self.job.created_at_ms))
    }

    /// The record as it is true right now: a job a worker was meant to be
    /// holding, with nobody holding it, is `interrupted`.
    ///
    /// A `queued` job counts only once a worker has written its pid, which it
    /// does after it has everything it needs to run. Before that the job has
    /// simply not been picked up yet, and a client that read it as interrupted
    /// would start a second worker on top of one still starting, or on top of
    /// one that stood down because another worker owns the same reference.
    pub fn status(&self) -> PullStatus {
        let mut status = self.stored_status();
        let expects_worker = match status.state {
            PullState::Running => true,
            PullState::Queued => status.pid.is_some(),
            _ => false,
        };
        if expects_worker && !self.worker_alive() {
            status.state = PullState::Interrupted;
        }
        status
    }

    /// Whether the job is waiting for a worker that is not coming: queued with
    /// no pid written, nobody holding the lock, and `grace_ms` past its last
    /// write, by which time a worker on its way would have claimed it.
    ///
    /// This is the one state the liveness rule cannot speak for. A worker writes
    /// its pid as soon as it holds the job, so `queued` without one means
    /// nothing has taken the job yet; only time tells a job still being picked
    /// up from one whose worker died on the way.
    pub fn abandoned(&self, now_ms: i64, grace_ms: i64) -> bool {
        let status = self.stored_status();
        status.state == PullState::Queued
            && status.pid.is_none()
            && now_ms.saturating_sub(status.updated_at_ms) >= grace_ms
            && !self.worker_alive()
    }

    /// Write `status` atomically, so a reader sees the old record or the new one
    /// and never half of either.
    ///
    /// A job whose directory has been swept is reported gone rather than
    /// recreated: an atomic write makes its parents, and a worker writing into a
    /// removed job would leave a directory no listing can see.
    pub fn write_status(&self, status: &PullStatus) -> Result<(), PullError> {
        self.guard_write(|| {
            persistence::write_json_atomic(&self.path.join(STATUS_FILE), status)?;
            Ok(())
        })
    }

    /// Read the stored record, hand it to `change`, and write it back stamped
    /// `now`. Saves the caller from carrying the record between writes; the
    /// worker is the only writer, so no two of these can interleave.
    pub fn update_status(
        &self,
        now: i64,
        change: impl FnOnce(&mut PullStatus),
    ) -> Result<PullStatus, PullError> {
        let mut status = self.stored_status();
        change(&mut status);
        status.updated_at_ms = now;
        self.write_status(&status)?;
        Ok(status)
    }

    /// Append `kind` to the history, stamped `now`. A file whose last line was
    /// torn off mid-write gets its newline back first, so the damage stays on
    /// the line it happened to.
    pub fn append(&self, kind: PullEventKind, now: i64) -> Result<(), PullError> {
        let event = PullEvent { at_ms: now, kind };
        let mut line = serde_json::to_vec(&event).map_err(StoreError::Encode)?;
        line.push(b'\n');
        let path = self.path.join(EVENTS_FILE);
        let unterminated = ends_mid_line(&path);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        if unterminated {
            file.write_all(b"\n")?;
        }
        file.write_all(&line)?;
        Ok(())
    }

    /// The history, oldest first. A line that will not decode is skipped rather
    /// than sinking the rest of the file, and that includes a line that is not
    /// even valid text.
    pub fn events(&self) -> Vec<PullEvent> {
        let Ok(bytes) = fs::read(self.path.join(EVENTS_FILE)) else {
            return Vec::new();
        };
        bytes
            .split(|byte| *byte == b'\n')
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect()
    }

    /// What a client has asked the worker to do, if anything.
    pub fn control(&self) -> Option<PullControl> {
        fs::read_to_string(self.path.join(CONTROL_FILE))
            .ok()
            .as_deref()
            .and_then(PullControl::parse)
    }

    /// Ask the worker for `control`.
    pub fn request(&self, control: PullControl) -> Result<(), PullError> {
        self.guard_write(|| {
            persistence::write_atomic(&self.path.join(CONTROL_FILE), control.as_str().as_bytes())?;
            Ok(())
        })
    }

    /// Drop the control the worker has honoured, leaving a later one alone: a
    /// cancel that arrived while a pause was being honoured is still waiting to
    /// be read, and deleting it would lose it silently.
    pub fn clear_control(&self, honoured: PullControl) -> Result<(), PullError> {
        if self.control() != Some(honoured) {
            return Ok(());
        }
        match fs::remove_file(self.path.join(CONTROL_FILE)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Delete the job's directory. The weights it fetched are not touched: they
    /// belong to the model store, not to the job.
    pub fn remove(&self) -> Result<(), PullError> {
        fs::remove_dir_all(&self.path)?;
        Ok(())
    }

    /// Refuse to write into a job that has been swept, and undo the write when
    /// the sweep lands between the check and it.
    ///
    /// An atomic write makes the directories it needs, so a write racing a
    /// sweep would otherwise leave a directory holding a record and no
    /// descriptor: invisible to every listing, and therefore never collected.
    fn guard_write(&self, write: impl FnOnce() -> Result<(), PullError>) -> Result<(), PullError> {
        self.require_job()?;
        write()?;
        match self.require_job() {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_dir_all(&self.path);
                Err(error)
            }
        }
    }

    fn require_job(&self) -> Result<(), PullError> {
        match self.path.join(JOB_FILE).is_file() {
            true => Ok(()),
            false => Err(PullError::NotFound(self.job.id.clone())),
        }
    }
}

/// Take an exclusive lock on `path`, creating it if it is not there, or `None`
/// when someone else holds it. The lock lives with the returned handle.
pub fn take_lock(path: &Path) -> Result<Option<PullLock>, PullError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(PullLock { file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Whether `path` holds bytes that do not end in a newline, so the next append
/// would glue itself onto a line torn off mid-write.
fn ends_mid_line(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .filter(|bytes| !bytes.is_empty())
        .is_some_and(|bytes| bytes.last() != Some(&b'\n'))
}

/// The last segment of `path`, for naming a directory that has no descriptor.
fn name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("pull")
        .to_owned()
}

/// `reference` as the readable half of a job id: lowercase, one dash between
/// runs of anything else, and short enough to keep the id typeable. Deliberately
/// not the artifact store's slug, which has its own rules for its own names.
fn reference_slug(reference: &str) -> String {
    let mut slug = String::with_capacity(reference.len().min(SLUG_LIMIT));
    for character in reference.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= SLUG_LIMIT {
            break;
        }
    }
    let trimmed = slug.trim_matches('-');
    match trimmed.is_empty() {
        true => "model".to_owned(),
        false => trimmed.to_owned(),
    }
}
