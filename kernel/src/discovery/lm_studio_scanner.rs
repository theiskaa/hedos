//! Scans LM Studio's model tree: each root is walked for `.gguf` weight files
//! (skipping multimodal projectors), which are grouped into models by
//! [`discovered_models`]. The repo label is the `<publisher>/<model>` prefix of a
//! file's path relative to the root.

use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::{discovered_models, is_mmproj_name};
use crate::discovery::scanner::{ScanResult, StoreScanner};
use crate::records::SourceKind;

/// A scanner over one or more LM Studio model roots.
pub struct LMStudioScanner {
    roots: Vec<PathBuf>,
}

impl LMStudioScanner {
    /// A scanner over the given roots (a missing root is skipped).
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }

    /// A scanner over a single root.
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self::new(vec![root.into()])
    }

    fn scan_root(&self, root: &Path, result: &mut ScanResult) {
        if !root.exists() {
            return;
        }
        let mut ggufs = Vec::new();
        if collect_ggufs(root, &mut ggufs).is_err() {
            if !result.failed_kinds.contains(&SourceKind::lm_studio()) {
                result.failed_kinds.push(SourceKind::lm_studio());
            }
            return;
        }
        let (models, issues) =
            discovered_models(&ggufs, &SourceKind::lm_studio(), |path| repo_of(path, root));
        result.discovered.extend(models);
        result.issues.extend(issues);
    }
}

impl StoreScanner for LMStudioScanner {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::lm_studio()]
    }

    fn scan(&self) -> ScanResult {
        let mut result = ScanResult::default();
        for root in &self.roots {
            self.scan_root(root, &mut result);
        }
        result
    }
}

/// The `<publisher>/<model>` repo of a file relative to `root`: everything but the
/// final path component, when the file is at least three components deep.
fn repo_of(path: &Path, root: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.len() >= 3 {
        Some(parts[..parts.len() - 1].join("/"))
    } else {
        None
    }
}

/// Recursively collect `.gguf` weight files (non-projector, regular) under `dir`,
/// with sizes (following symlinks). A top-level read error propagates; a
/// subdirectory that can't be read is skipped.
fn collect_ggufs(dir: &Path, into: &mut Vec<(PathBuf, i64)>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                let _ = collect_ggufs(&path, into);
            }
            _ if is_gguf_weight(&path) => {
                if let Ok(meta) = std::fs::metadata(&path)
                    && meta.is_file()
                {
                    into.push((path, meta.len() as i64));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_gguf_weight(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if is_mmproj_name(name) {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
}
