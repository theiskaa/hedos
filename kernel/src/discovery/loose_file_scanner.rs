//! Scans loose directories (Downloads, a Models folder) up to a shallow depth
//! for models a user dropped in by hand: a `config.json`+`safetensors` folder
//! becomes one bundle model, loose `.gguf` files group into models, and a
//! GGML-magic `.bin` is a whisper transcription model.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::{discovered_models, is_mmproj_name};
use crate::discovery::modality_hints::{self, Hint};
use crate::discovery::scanner::{DiscoveredModel, ScanResult, StoreScanner};
use crate::records::{ExecutionMode, ModelSource, SourceKind};
use crate::resolution::has_ggml_magic;

/// How deep to sweep below each root (0 = the root's own entries).
const MAX_DEPTH: usize = 2;

/// A scanner over loose model directories.
pub struct LooseFileScanner {
    directories: Vec<PathBuf>,
    user_directories: Vec<PathBuf>,
}

impl LooseFileScanner {
    /// A scanner over the given directories (a missing one is skipped).
    pub fn new(directories: Vec<PathBuf>) -> Self {
        Self {
            directories,
            user_directories: Vec::new(),
        }
    }

    /// A scanner over a single directory.
    pub fn single(directory: impl Into<PathBuf>) -> Self {
        Self::new(vec![directory.into()])
    }

    /// A scanner over standard `directories` (missing → skipped) plus
    /// `user_directories` (missing → a scan failure).
    pub fn with_user_directories(
        directories: Vec<PathBuf>,
        user_directories: Vec<PathBuf>,
    ) -> Self {
        Self {
            directories,
            user_directories,
        }
    }

    fn scan_root(&self, dir: &Path, required: bool, result: &mut ScanResult) {
        if !dir.exists() {
            if required {
                mark_failed(result);
            }
            return;
        }
        if std::fs::read_dir(dir).is_err() {
            mark_failed(result);
            return;
        }
        self.sweep(dir, 0, result);
    }

    fn sweep(&self, dir: &Path, depth: usize, result: &mut ScanResult) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };

        let mut ggufs: Vec<(PathBuf, i64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if is_hidden(&path) {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => match folder_bundle(&path) {
                    Some(bundle) => result.discovered.push(bundle),
                    None => self.sweep(&path, depth + 1, result),
                },
                _ => {
                    let size = std::fs::metadata(&path)
                        .map(|meta| meta.len() as i64)
                        .unwrap_or(0);
                    if is_gguf_weight(&path) {
                        ggufs.push((path, size));
                    } else if is_ggml_bin(&path) {
                        result.discovered.push(whisper_model(&path, size));
                    }
                }
            }
        }

        let (models, issues) = discovered_models(&ggufs, &SourceKind::file(), |_| None);
        result.discovered.extend(models);
        result.issues.extend(issues);
    }
}

impl StoreScanner for LooseFileScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::file(), SourceKind::folder()]
    }

    fn scan(&self) -> ScanResult {
        let mut result = ScanResult::default();
        for dir in &self.directories {
            self.scan_root(dir, false, &mut result);
        }
        for dir in &self.user_directories {
            self.scan_root(dir, true, &mut result);
        }
        result
    }
}

/// A `config.json` + `safetensors` directory as a single folder-bundle model, or
/// `None` if it isn't one.
fn folder_bundle(dir: &Path) -> Option<DiscoveredModel> {
    let mut names = BTreeSet::new();
    let mut entries: Vec<(PathBuf, i64)> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if is_hidden(&path) {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.insert(name.to_owned());
        }
        let size = std::fs::metadata(&path)
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        entries.push((path, size));
    }

    let has_safetensors = entries.iter().any(|(path, _)| is_safetensors(path));
    if !names.contains("config.json") || !has_safetensors {
        return None;
    }

    let mut hint = modality_hints::from_config_json(&dir.join("config.json"))
        .unwrap_or_else(|| Hint::unknown(ExecutionMode::Sync));
    if names.contains("model_index.json") {
        hint = modality_hints::from_model_index(&dir.join("model_index.json"));
    }

    let total: i64 = entries.iter().map(|(_, size)| size).sum();
    // First of equal-size safetensors wins (strict `>`) — `max_by_key` would
    // keep the last.
    let mut largest: Option<(&Path, i64)> = None;
    for (path, size) in entries.iter().filter(|(path, _)| is_safetensors(path)) {
        if largest.is_none_or(|(_, best)| *size > best) {
            largest = Some((path, *size));
        }
    }
    let largest = largest.map(|(path, _)| display(path));

    let mut model = DiscoveredModel::new(
        dir.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        ModelSource::new(SourceKind::folder(), &display(dir)),
    );
    model.modality_hint = hint.modality;
    model.capabilities_hint = hint.capabilities;
    model.execution_hint = hint.execution;
    model.footprint_bytes = total;
    model.primary_weight_path = largest;
    model.context_length_hint = hint.context_length;
    Some(model)
}

fn whisper_model(path: &Path, size: i64) -> DiscoveredModel {
    let hint = modality_hints::whisper_bin_hint();
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let mut model =
        DiscoveredModel::new(name, ModelSource::new(SourceKind::file(), &display(path)));
    model.modality_hint = hint.modality;
    model.capabilities_hint = hint.capabilities;
    model.execution_hint = hint.execution;
    model.footprint_bytes = size;
    model.primary_weight_path = Some(display(path));
    model
}

fn mark_failed(result: &mut ScanResult) {
    for kind in [SourceKind::file(), SourceKind::folder()] {
        if !result.failed_kinds.contains(&kind) {
            result.failed_kinds.push(kind);
        }
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn is_gguf_weight(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    !is_mmproj_name(name) && has_extension_ignoring_case(path, "gguf")
}

fn is_ggml_bin(path: &Path) -> bool {
    has_extension_ignoring_case(path, "bin") && has_ggml_magic(path)
}

/// A case-sensitive `.safetensors` extension check.
fn is_safetensors(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("safetensors")
}

fn has_extension_ignoring_case(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
