//! The terminal-job history: a `jobs.json` store of concluded jobs, newest
//! first, trimmed to a limit. Only terminal jobs are ever recorded.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::persistence::{self, StoreError};

use super::job::Job;

const STORE_FILE: &str = "jobs.json";
const SCHEMA_VERSION: u32 = 1;
const DEFAULT_LIMIT: usize = 50;

#[derive(Serialize, Deserialize)]
struct Envelope {
    schema_version: u32,
    jobs: Vec<Job>,
}

/// A bounded, newest-first store of concluded jobs. Lazily loaded; a corrupt
/// `jobs.json` is quarantined and the store starts empty rather than failing.
pub struct JobHistoryStore {
    directory: PathBuf,
    limit: usize,
    jobs: Vec<Job>,
    loaded: bool,
}

impl JobHistoryStore {
    /// A store rooted at `directory`, keeping at most `limit` jobs (floored at 1).
    pub fn new(directory: &Path, limit: usize) -> Self {
        Self {
            directory: directory.to_path_buf(),
            limit: limit.max(1),
            jobs: Vec::new(),
            loaded: false,
        }
    }

    /// A store with the default limit of 50 jobs.
    pub fn with_default_limit(directory: &Path) -> Self {
        Self::new(directory, DEFAULT_LIMIT)
    }

    /// The current retention limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Set the retention limit (floored at 1). Takes effect on the next `record`.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    /// Record a concluded `job`, replacing any prior entry with the same id,
    /// re-sorting newest-first, and trimming to the limit.
    pub fn record(&mut self, job: Job) -> Result<(), StoreError> {
        self.load_if_needed();
        self.jobs.retain(|existing| existing.id != job.id);
        self.jobs.push(job);
        self.jobs
            .sort_by(|a, b| (b.submitted_at, &b.id).cmp(&(a.submitted_at, &a.id)));
        self.jobs.truncate(self.limit);
        self.save()
    }

    /// Every recorded job, newest first.
    pub fn list(&mut self) -> &[Job] {
        self.load_if_needed();
        &self.jobs
    }

    /// The recorded job with `id`, if any.
    pub fn get(&mut self, id: &str) -> Option<Job> {
        self.load_if_needed();
        self.jobs.iter().find(|job| job.id == id).cloned()
    }

    fn store_file(&self) -> PathBuf {
        self.directory.join(STORE_FILE)
    }

    fn load_if_needed(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        // A missing store, a corrupt one (quarantined inside read_json), or an io
        // error all leave the store empty, matching the Swift best-effort load.
        if let Ok(Some(envelope)) = persistence::read_json::<Envelope>(&self.store_file()) {
            self.jobs = envelope.jobs;
        }
    }

    fn save(&self) -> Result<(), StoreError> {
        let envelope = Envelope {
            schema_version: SCHEMA_VERSION,
            jobs: self.jobs.clone(),
        };
        persistence::write_json_atomic(&self.store_file(), &envelope)
    }
}
