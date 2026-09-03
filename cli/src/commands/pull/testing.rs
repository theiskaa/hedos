//! Scaffolding for the pull command's tests: a temporary store and the plans and
//! records the commands read.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use kernel::install::plan::{InstallPlan, InstallPlanFile};
use kernel::install::provider::InstallProviderId;
use kernel::install::pulls::{PullJobDir, PullStore};

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
