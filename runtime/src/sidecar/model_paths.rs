//! Resolving a model record to the filesystem paths a Python sidecar loads from:
//! the sandbox root to grant read access to, and the snapshot directory holding
//! the weights (a Hugging Face revision snapshot, or the model root itself).

use std::collections::BTreeSet;
use std::path::Path;

use kernel::records::{ModelRecord, SourceKind};

/// The paths a sidecar needs for a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarModelPaths {
    /// The directory the sandbox grants the sidecar read access to.
    pub sandbox_root: String,
    /// The directory the weights are loaded from.
    pub snapshot: String,
}

impl SidecarModelPaths {
    /// Resolve `record`'s source path to its sandbox root and snapshot. For a
    /// Hugging Face cache with a revision ref, the snapshot is that revision's
    /// directory when it exists; otherwise both are the (symlink-resolved) root.
    pub fn resolve(record: &ModelRecord) -> Self {
        let base = kernel::fs::expand_tilde_env(&record.source.path);
        let root = std::fs::canonicalize(&base).unwrap_or(base);
        if record.source.kind == SourceKind::huggingface_cache()
            && let Some(reference) = &record.source.reference
        {
            let snapshot = root.join("snapshots").join(reference);
            if snapshot.exists() {
                return Self {
                    sandbox_root: path_string(&root),
                    snapshot: path_string(&snapshot),
                };
            }
        }
        let root = path_string(&root);
        Self {
            sandbox_root: root.clone(),
            snapshot: root,
        }
    }
}

/// Weight-file extensions that name a bundled speech voice.
const VOICE_WEIGHT_EXTENSIONS: &[&str] = &["safetensors", "pt", "bin", "npz"];

/// The speech voices bundled with `record`: the weight-file stems in its
/// snapshot's `voices/` directory, de-duplicated and sorted. Empty when the
/// model bundles no `voices/` directory (or it cannot be read).
pub fn speech_voices(record: &ModelRecord) -> Vec<String> {
    let snapshot = SidecarModelPaths::resolve(record).snapshot;
    let directory = Path::new(&snapshot).join("voices");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        let path = Path::new(file_name);
        let is_weight = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| VOICE_WEIGHT_EXTENSIONS.contains(&ext.to_lowercase().as_str()));
        if !is_weight {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            && !stem.is_empty()
        {
            names.insert(stem.to_owned());
        }
    }
    names.into_iter().collect()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use kernel::records::{Modality, ModelSource};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hedos-sidecar-paths-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(kind: SourceKind, path: &str, reference: Option<&str>) -> ModelRecord {
        let mut source = ModelSource::new(kind, path);
        source.reference = reference.map(str::to_owned);
        ModelRecord::new("m", Modality::text(), Vec::new(), source)
    }

    #[test]
    fn a_plain_model_uses_its_root_for_both() {
        let dir = temp_dir("plain");
        let paths =
            SidecarModelPaths::resolve(&record(SourceKind::folder(), dir.to_str().unwrap(), None));
        assert_eq!(paths.sandbox_root, paths.snapshot);
        assert!(
            paths.snapshot.ends_with("plain") || paths.snapshot.contains("hedos-sidecar-paths")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn speech_voices_lists_sorted_deduped_weight_stems() {
        let dir = temp_dir("voices");
        let voices = dir.join("voices");
        std::fs::create_dir_all(&voices).unwrap();
        for file in [
            "af_heart.safetensors",
            "am_onyx.pt",
            "bf_emma.bin",
            "cf_yue.npz",
            ".hidden.safetensors", // dot-prefixed → skipped
            "readme.txt",          // non-weight → skipped
        ] {
            std::fs::write(voices.join(file), b"x").unwrap();
        }
        let names = speech_voices(&record(SourceKind::folder(), dir.to_str().unwrap(), None));
        assert_eq!(names, ["af_heart", "am_onyx", "bf_emma", "cf_yue"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn speech_voices_is_empty_without_a_voices_directory() {
        let dir = temp_dir("no-voices");
        let names = speech_voices(&record(SourceKind::folder(), dir.to_str().unwrap(), None));
        assert!(names.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hugging_face_cache_uses_the_revision_snapshot() {
        let dir = temp_dir("hf");
        let snapshot = dir.join("snapshots").join("abc123");
        std::fs::create_dir_all(&snapshot).unwrap();
        let paths = SidecarModelPaths::resolve(&record(
            SourceKind::huggingface_cache(),
            dir.to_str().unwrap(),
            Some("abc123"),
        ));
        assert!(paths.snapshot.ends_with("snapshots/abc123"));
        assert_ne!(paths.sandbox_root, paths.snapshot);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hugging_face_cache_without_the_snapshot_falls_back_to_the_root() {
        let dir = temp_dir("hf-missing");
        let paths = SidecarModelPaths::resolve(&record(
            SourceKind::huggingface_cache(),
            dir.to_str().unwrap(),
            Some("missing"),
        ));
        assert_eq!(paths.sandbox_root, paths.snapshot);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tilde_path_expands_against_home() {
        // A non-existent ~-path can't be canonicalized, so it stays tilde-expanded.
        let paths = SidecarModelPaths::resolve(&record(
            SourceKind::folder(),
            "~/no-such-hedos-model",
            None,
        ));
        if std::env::var("HOME").is_ok() {
            assert!(!paths.snapshot.starts_with('~'));
        }
    }
}
