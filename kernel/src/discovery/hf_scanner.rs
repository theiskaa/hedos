//! Scans a Hugging Face hub cache: each `models--<org>--<repo>` directory's
//! current snapshot is inspected (`config.json` / `model_index.json` / a bare
//! GGUF) for a modality hint, its blobs summed for the footprint, and its
//! shard/blob completeness checked to flag a still-downloading model.
//!
//! The diffusers `model_index.json` path yields only a generic job hint until the
//! pipeline-family registry it needs is ported.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::is_mmproj_name;
use crate::discovery::gguf_shards::{group, parse, shard_filename};
use crate::discovery::modality_hints::{self, Hint};
use crate::discovery::scanner::{DiscoveredModel, ScanResult, StoreScanner};
use crate::records::{ExecutionMode, JsonValue, Modality, ModelSource, SourceKind};
use crate::resolution::has_ggml_magic;

/// Filenames that, alongside a text/unknown model, mark a sentence-transformers
/// embedding model.
const SENTENCE_TRANSFORMERS_MARKERS: [&str; 2] = ["config_sentence_transformers.json", "1_Pooling"];

/// A scanner over one or more Hugging Face hub cache roots.
pub struct HFCacheScanner {
    roots: Vec<PathBuf>,
    user_roots: Vec<PathBuf>,
}

impl HFCacheScanner {
    /// A scanner over the given standard cache `roots` (a missing root is skipped).
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            user_roots: Vec::new(),
        }
    }

    /// A scanner over a single root.
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self::new(vec![root.into()])
    }

    /// A scanner over standard `roots` (missing → skipped) plus `user_roots`
    /// (missing → a scan failure, since the user pointed at them explicitly).
    pub fn with_user_roots(roots: Vec<PathBuf>, user_roots: Vec<PathBuf>) -> Self {
        Self { roots, user_roots }
    }

    fn scan_root(&self, root: &Path, required: bool, result: &mut ScanResult) {
        if !root.exists() {
            if required {
                mark_failed(result);
            }
            return;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            mark_failed(result);
            return;
        };

        for entry in entries.flatten() {
            let dir = entry.path();
            let Some(dir_name) = dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(rest) = dir_name.strip_prefix("models--") else {
                continue;
            };
            let repo = rest.replace("--", "/");

            let Some((snapshot, revision)) = current_snapshot(&dir) else {
                result
                    .issues
                    .push(format!("hf-cache: {repo} has no usable snapshot"));
                continue;
            };

            let names = snapshot_file_names(&snapshot);
            let mut diagnostics = Vec::new();
            let hint = resolve_hint(&snapshot, &names, &mut diagnostics);

            let downloading = has_incomplete_blobs(&dir.join("blobs"))
                || index_references_missing_shard(&snapshot, &names)
                || gguf_shards_incomplete(&snapshot, &names);

            // Last non-empty path segment (Swift's `split` omits empty ones, so a
            // trailing slash doesn't yield an empty name).
            let name = repo
                .rsplit('/')
                .find(|segment| !segment.is_empty())
                .unwrap_or(&repo)
                .to_owned();
            let mut source = ModelSource::new(SourceKind::huggingface_cache(), &display(&dir));
            source.repo = Some(repo);
            source.reference = Some(revision);

            let mut discovered = DiscoveredModel::new(name, source);
            discovered.modality_hint = hint.modality;
            discovered.capabilities_hint = hint.capabilities;
            discovered.execution_hint = hint.execution;
            discovered.footprint_bytes = directory_bytes(&dir.join("blobs"));
            discovered.primary_weight_path = largest_weight(&snapshot);
            discovered.diagnostics = diagnostics;
            discovered.context_length_hint = hint.context_length;
            discovered.downloading = downloading;
            result.discovered.push(discovered);
        }
    }
}

impl StoreScanner for HFCacheScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::huggingface_cache()]
    }

    fn scan(&self) -> ScanResult {
        let mut result = ScanResult::default();
        for root in &self.roots {
            self.scan_root(root, false, &mut result);
        }
        for root in &self.user_roots {
            self.scan_root(root, true, &mut result);
        }
        result
    }
}

fn mark_failed(result: &mut ScanResult) {
    let kind = SourceKind::huggingface_cache();
    if !result.failed_kinds.contains(&kind) {
        result.failed_kinds.push(kind);
    }
}

/// Pick the snapshot to represent a repo: `refs/main` if it points at a present
/// snapshot, else the most-recently-modified snapshot directory.
fn current_snapshot(repo_dir: &Path) -> Option<(PathBuf, String)> {
    let snapshots = repo_dir.join("snapshots");

    if let Ok(revision) = std::fs::read_to_string(repo_dir.join("refs/main")) {
        let trimmed = revision.trim();
        if !trimmed.is_empty() {
            let snapshot = snapshots.join(trimmed);
            if snapshot.exists() {
                return Some((snapshot, trimmed.to_owned()));
            }
        }
    }

    let mut newest: Option<(PathBuf, std::time::SystemTime)> = None;
    for entry in std::fs::read_dir(&snapshots)
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        // Strict `>` keeps the first of equal-mtime snapshots, matching Swift's
        // `max(by: <)`.
        if newest.as_ref().is_none_or(|(_, best)| modified > *best) {
            newest = Some((entry.path(), modified));
        }
    }
    newest.map(|(path, _)| {
        let revision = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        (path, revision)
    })
}

fn snapshot_file_names(snapshot: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(snapshot).into_iter().flatten().flatten() {
        if let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
        {
            names.insert(name.to_owned());
        }
    }
    names
}

/// Determine the modality hint for a snapshot from its files, then apply the
/// sentence-transformers and missing-tokenizer refinements.
fn resolve_hint(snapshot: &Path, names: &BTreeSet<String>, diagnostics: &mut Vec<String>) -> Hint {
    let mut hint = if names.contains("model_index.json") {
        modality_hints::from_model_index(&snapshot.join("model_index.json"))
    } else if names.contains("config.json") {
        modality_hints::from_config_json(&snapshot.join("config.json"))
            .unwrap_or_else(|| Hint::unknown(ExecutionMode::Sync))
    } else if names.iter().any(|name| is_gguf(name)) {
        modality_hints::gguf_hint()
    } else {
        diagnostics.push("no config.json or model_index.json in snapshot".to_owned());
        Hint::unknown(ExecutionMode::Sync)
    };

    let text = Some(Modality::text());
    if (hint.modality.is_none() || hint.modality == text)
        && names
            .iter()
            .any(|name| SENTENCE_TRANSFORMERS_MARKERS.contains(&name.as_str()))
    {
        let mut embedding = modality_hints::embedding_hint();
        embedding.context_length = hint.context_length;
        hint = embedding;
    }

    if hint.modality == text
        && !names
            .iter()
            .any(|name| name.starts_with("tokenizer") || name == "vocab.json")
    {
        diagnostics.push("no tokenizer found".to_owned());
    }

    hint
}

fn has_incomplete_blobs(blobs: &Path) -> bool {
    for entry in std::fs::read_dir(blobs).into_iter().flatten().flatten() {
        let is_incomplete = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".incomplete"));
        if is_incomplete
            && entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn gguf_shards_incomplete(snapshot: &Path, names: &BTreeSet<String>) -> bool {
    let ggufs: Vec<(PathBuf, i64)> = names
        .iter()
        .filter(|name| is_gguf(name))
        .map(|name| (snapshot.join(name), 0))
        .collect();
    let (groups, _) = group(&ggufs);
    groups.iter().any(|shard_group| !shard_group.complete())
}

/// Whether a `*.safetensors.index.json` weight map references a shard whose file
/// (resolving symlinks into the blob store) is missing — the sign of a partial
/// safetensors download.
fn index_references_missing_shard(snapshot: &Path, names: &BTreeSet<String>) -> bool {
    for index_name in names
        .iter()
        .filter(|name| name.ends_with(".safetensors.index.json"))
    {
        let Ok(bytes) = std::fs::read(snapshot.join(index_name)) else {
            continue;
        };
        let Ok(JsonValue::Object(json)) = serde_json::from_slice::<JsonValue>(&bytes) else {
            continue;
        };
        let Some(JsonValue::Object(weight_map)) = json.get("weight_map") else {
            continue;
        };
        let shards: BTreeSet<&str> = weight_map.values().filter_map(JsonValue::as_str).collect();
        // `exists()` follows the snapshot's symlink into `blobs/`, so a dangling
        // link (a shard not yet downloaded) reads as missing.
        if shards.iter().any(|shard| !snapshot.join(shard).exists()) {
            return true;
        }
    }
    false
}

fn directory_bytes(dir: &Path) -> i64 {
    let mut total = 0;
    walk_bytes(dir, &mut total);
    total
}

fn walk_bytes(dir: &Path, total: &mut i64) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk_bytes(&path, total),
            // Follow symlinks for the size (a snapshot's weights are links into
            // `blobs/`); count only what resolves to a regular file.
            _ => {
                if let Ok(meta) = std::fs::metadata(&path)
                    && meta.is_file()
                {
                    *total += meta.len() as i64;
                }
            }
        }
    }
}

/// The largest weight file in a snapshot, resolved through its symlink. For a
/// GGUF shard set, the first shard's path is returned instead.
fn largest_weight(snapshot: &Path) -> Option<String> {
    let mut names = Vec::new();
    let mut best: Option<(PathBuf, i64)> = None;
    for entry in std::fs::read_dir(snapshot).into_iter().flatten().flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.push(name.to_owned());
        }
        if is_weight_file(&path) {
            let size = std::fs::metadata(&path)
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            if best.as_ref().is_none_or(|(_, best_size)| size > *best_size) {
                best = Some((path, size));
            }
        }
    }

    let (best_path, _) = best?;
    if let Some(shard) = best_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse)
    {
        let first = shard_filename(&shard.base, 1, shard.total);
        if names.contains(&first) {
            return Some(resolve(&snapshot.join(first)));
        }
    }
    Some(resolve(&best_path))
}

fn is_weight_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if is_mmproj_name(name) {
        return false;
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("safetensors" | "gguf") => true,
        Some("bin") => has_ggml_magic(path),
        _ => false,
    }
}

fn is_gguf(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gguf")
}

/// A path resolved through symlinks (falling back to itself), as a string.
fn resolve(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
