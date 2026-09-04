//! Scaffolding for the tests that read a pull's record: a temporary store, and
//! the plans and jobs to fill it with.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use kernel::install::event::InstallProgress;
use kernel::install::plan::{InstallPlan, InstallPlanFile};
use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{PullJobDir, PullLock, PullState, PullStatus, PullStore};

/// Keeps two directories made in the same nanosecond apart.
static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// A directory that deletes itself. Named by process and counter rather than by
/// thread, so two `cargo test` runs at once cannot land on the same path.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// A fresh directory under the system temporary directory.
    pub fn new(label: &str) -> Self {
        let unique = UNIQUE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hedos-pull-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create the temporary directory");
        Self { path }
    }

    /// The store of pull jobs inside it.
    pub fn store(&self) -> PullStore {
        PullStore::new(self.path.join("pulls"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A Hugging Face plan for `reference`, sized so progress figures have
/// something to divide by.
pub fn plan(reference: &str) -> InstallPlan {
    let mut plan = InstallPlan::new(
        InstallProviderId::huggingface(),
        reference,
        reference.rsplit('/').next().unwrap_or(reference),
        "/models/somewhere",
    );
    plan.files = vec![InstallPlanFile::new("model.gguf", Some(4_000_000_000))];
    plan.total_bytes = Some(4_000_000_000);
    plan.remaining_bytes = Some(4_000_000_000);
    plan
}

/// A job for `reference` in `store`, created at `now`.
pub fn job(store: &PullStore, reference: &str, now: i64) -> PullJobDir {
    store.create(&plan(reference), now).expect("create the job")
}

/// A job with a worker holding it, so the record under test is the only thing a
/// reader is going by.
pub struct Held {
    _directory: TempDir,
    pub job: PullJobDir,
    _lock: PullLock,
}

/// A held job in a directory named for `label`.
pub fn held(label: &str) -> Held {
    let directory = TempDir::new(label);
    let job = job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
    let lock = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    Held {
        _directory: directory,
        job,
        _lock: lock,
    }
}

/// A record in `state` and nothing else.
pub fn status(state: PullState) -> PullStatus {
    let mut status = PullStatus::queued(1_000);
    status.state = state;
    status
}

/// A running record that has transferred `downloaded` of `total`.
pub fn moved(downloaded: i64, total: Option<i64>, partial: bool) -> PullStatus {
    let mut status = status(PullState::Running);
    status.progress = InstallProgress {
        bytes_downloaded: downloaded,
        total_bytes: total,
        total_is_partial: partial,
        current_file: None,
    };
    status
}
