//! Shared helpers for the `runtime` integration tests. Centralizes the temp-dir
//! fixture every test file needs. Per the Rust Book, `tests/support/mod.rs` is
//! the canonical place for integration-test helper code — it is not compiled as
//! its own test binary, only pulled in via `mod support;`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory that removes itself when dropped. Built without
/// the `tempfile` crate to honor the project's minimal-dependency policy.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a fresh, process-unique temporary directory.
    pub fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("hedos-test-{pid}-{nanos}-{unique}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path to `name` inside this directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
