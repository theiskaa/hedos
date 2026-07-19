//! A cache over [`identify`](crate::resolution::identify) keyed by an on-disk
//! freshness signature. Identifying a file model reads its GGUF/safetensors
//! header and stats its directory; over a large shelf that repeats on every
//! resolution pass. The cache skips it when a model's bytes are unchanged.
//!
//! The cheap source kinds (builtin/endpoint/ollama) are never cached — their
//! identification is a fixed profile or a small manifest read, so a cache entry
//! would cost more than it saves.
//!
//! The freshness signature is depth-1: it folds the model path and its immediate
//! children, not files nested deeper (an HF-cache `snapshots/<ref>/config.json`
//! or a diffusers `transformer/config.json`). An in-place edit *below* the top
//! level is therefore not noticed. Adding, removing, or replacing an immediate
//! child (the common re-download shape) is caught.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::records::{ModelRecord, SourceKind};
use crate::resolution::identity::{IdentifiedModel, identify as run_identify};

struct Entry {
    mtime: SystemTime,
    size: i64,
    identified: IdentifiedModel,
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, Entry>,
    hits: usize,
}

/// A freshness-keyed cache of identification results, safe to share across
/// threads. A cache miss (or any lock failure) falls back to a direct
/// [`identify`](crate::resolution::identify), so the cache is only ever an
/// optimization. A changed top-level mtime or size (an added/removed/replaced
/// immediate child, or a rewritten file) invalidates the entry; see the module
/// docs for the depth-1 limitation.
#[derive(Default)]
pub struct IdentificationCache {
    state: Mutex<CacheState>,
}

impl IdentificationCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of cache hits so far (for diagnostics/tests).
    pub fn hit_count(&self) -> usize {
        self.state.lock().map_or(0, |state| state.hits)
    }

    /// Identify `record`, returning a cached result when the model's bytes are
    /// unchanged since the last identification.
    pub fn identify(&self, record: &ModelRecord) -> IdentifiedModel {
        if is_uncached(&record.source.kind) {
            return run_identify(record);
        }
        let key = cache_key(record);
        let Some((mtime, size)) = freshness_signature(Path::new(&record.source.path)) else {
            return run_identify(record);
        };
        if let Ok(mut state) = self.state.lock()
            && let Some(entry) = state.entries.get(&key)
            && entry.mtime == mtime
            && entry.size == size
        {
            let identified = entry.identified.clone();
            state.hits += 1;
            return identified;
        }
        // Identify outside the lock — it does filesystem I/O — then record it.
        let identified = run_identify(record);
        if let Ok(mut state) = self.state.lock() {
            state.entries.insert(
                key,
                Entry {
                    mtime,
                    size,
                    identified: identified.clone(),
                },
            );
        }
        identified
    }
}

/// The kinds whose identification is too cheap to be worth caching. Matched on the
/// kind's string to avoid allocating a `SourceKind` (a `String` newtype) per check
/// on the resolution hot path.
fn is_uncached(kind: &SourceKind) -> bool {
    matches!(kind.as_str(), "builtin" | "endpoint" | "ollama")
}

/// The cache key for `record`. `identify` reads not just the path but — for
/// HF-cache/diffusers records — the source `reference` (which snapshot) and `repo`
/// (the pipeline repo hint), so two revisions sharing a path must not collide.
/// Keying on the path alone would collide, so these fields are included to prevent
/// a different revision being served the wrong identification.
fn cache_key(record: &ModelRecord) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        record.source.path,
        record.source.reference.as_deref().unwrap_or(""),
        record.source.repo.as_deref().unwrap_or(""),
    )
}

/// A `(mtime, size)` signature that changes whenever a model's bytes change. For
/// a file it is the file's own modification time and length; for a directory
/// (a multi-file model) it is the newest child mtime and the total size across
/// the directory and its immediate children — enough to notice a re-download or
/// an added/replaced shard without hashing anything.
fn freshness_signature(path: &Path) -> Option<(SystemTime, i64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let size = metadata.len() as i64;
    if !metadata.is_dir() {
        return Some((mtime, size));
    }
    let mut latest = mtime;
    let mut total = size;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(child) = entry.metadata() else {
                continue;
            };
            if let Ok(child_mtime) = child.modified()
                && child_mtime > latest
            {
                latest = child_mtime;
            }
            // Wrapping add: the total is an identity signature, not a real byte
            // count, so overflow only needs to stay consistent.
            total = total.wrapping_add(child.len() as i64);
        }
    }
    Some((latest, total))
}
