//! Deleting an installed model: what a deletion would touch (a preview) and the
//! pure path/size logic behind it. The actual removal — trashing files or asking
//! the Ollama daemon to delete a tag — is driven by the runtime crate.

use std::fs;
use std::path::{Path, PathBuf};

use crate::discovery::gguf_shards;
use crate::records::{ModelRecord, ModelState, SourceKind};

/// Why a model could not be deleted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemovalError {
    /// The model's kind is not something hedos can delete.
    #[error("{}", not_deletable_message(kind))]
    NotDeletable {
        /// The kind that can't be deleted.
        kind: SourceKind,
    },
    /// The model is generating right now.
    #[error("{name} is answering right now. Stop generation, then delete.")]
    ModelBusy {
        /// The model's name.
        name: String,
    },
    /// The model is still downloading.
    #[error("{name} is still downloading. Cancel the download first, then delete.")]
    StillDownloading {
        /// The model's name.
        name: String,
    },
    /// The Ollama daemon isn't available to delete its model.
    #[error("{0}")]
    DaemonUnavailable(String),
    /// The daemon refused or failed the delete.
    #[error("{0}")]
    DaemonDeleteFailed(String),
    /// Moving a file to the trash failed.
    #[error("Couldn't move {path} to the trash: {reason}")]
    TrashFailed {
        /// The path that couldn't be trashed.
        path: String,
        /// Why it failed.
        reason: String,
    },
}

fn not_deletable_message(kind: &SourceKind) -> String {
    if kind == &SourceKind::builtin() {
        "The built-in model ships with the OS and can't be deleted.".to_owned()
    } else if kind == &SourceKind::endpoint() {
        "Server models are connections, not files. Remove them from the servers list.".to_owned()
    } else {
        format!("{} models can't be deleted.", kind.as_str())
    }
}

/// What deleting a model would touch: the files (or a daemon delete), and an
/// estimate of the space it would free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDeletionPreview {
    /// The model's stable id.
    pub model_id: String,
    /// The name to show.
    pub name: String,
    /// The model's source kind.
    pub kind: SourceKind,
    /// The filesystem paths that would be removed (empty for a daemon delete).
    pub paths: Vec<String>,
    /// The estimated bytes freed.
    pub bytes_estimate: i64,
    /// Whether the delete goes through the Ollama daemon.
    pub via_daemon: bool,
    /// Whether the model's weights are already missing from disk.
    pub missing: bool,
}

/// The outcome of a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDeletionReport {
    /// The model's stable id.
    pub model_id: String,
    /// The name shown.
    pub name: String,
    /// The model's source kind.
    pub kind: SourceKind,
    /// The paths that were trashed.
    pub trashed_paths: Vec<String>,
    /// The estimated bytes freed.
    pub freed_bytes_estimate: i64,
    /// Whether the delete went through the Ollama daemon.
    pub daemon_deleted: bool,
}

/// Whether a model can be deleted (built-in and endpoint models can't).
pub fn is_deletable(record: &ModelRecord) -> bool {
    let kind = &record.source.kind;
    kind != &SourceKind::builtin() && kind != &SourceKind::endpoint()
}

/// Preview what deleting `record` would do.
pub fn preview(record: &ModelRecord) -> ModelDeletionPreview {
    let missing = record.state == ModelState::Missing;
    let is_ollama = record.source.kind == SourceKind::ollama();
    let via_daemon = !missing && is_ollama;
    let paths: Vec<String> = if is_ollama {
        Vec::new()
    } else {
        removable_paths(record)
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    };
    let bytes_estimate = if missing {
        on_disk_bytes(&paths)
    } else {
        record
            .footprint_mb
            .unwrap_or(0)
            .max(0)
            .saturating_mul(1 << 20)
    };
    ModelDeletionPreview {
        model_id: record.id.clone(),
        name: record.display_name().to_owned(),
        kind: record.source.kind.clone(),
        paths,
        bytes_estimate,
        via_daemon,
        missing,
    }
}

/// The files that removing `record` would delete (non-Ollama models).
pub fn removable_paths(record: &ModelRecord) -> Vec<PathBuf> {
    let source = PathBuf::from(&record.source.path);
    let kind = &record.source.kind;
    if kind == &SourceKind::huggingface_cache() || kind == &SourceKind::folder() {
        if source.exists() {
            vec![source]
        } else {
            vec![]
        }
    } else if kind == &SourceKind::lm_studio() || kind == &SourceKind::file() {
        shard_group(&source)
    } else {
        vec![]
    }
}

/// The full shard set a single-file model belongs to (or just the file when it
/// isn't a shard).
fn shard_group(path: &Path) -> Vec<PathBuf> {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(shard) = gguf_shards::parse(&filename) else {
        return if path.exists() {
            vec![path.to_path_buf()]
        } else {
            vec![]
        };
    };
    let Some(directory) = path.parent() else {
        return vec![];
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return vec![];
    };
    let mut group: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            gguf_shards::parse(&name)
                .filter(|candidate| candidate.base == shard.base && candidate.total == shard.total)
                .map(|_| entry.path())
        })
        .collect();
    group.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    group
}

fn on_disk_bytes(paths: &[String]) -> i64 {
    paths.iter().fold(0i64, |total, path| {
        let size = fs::metadata(path)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0);
        total.saturating_add(size.max(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Modality, ModelRecord, ModelSource};

    fn record(kind: SourceKind, path: &str) -> ModelRecord {
        ModelRecord::new(
            "Model",
            Modality::text(),
            Vec::new(),
            ModelSource::new(kind, path),
        )
    }

    #[test]
    fn built_in_and_endpoint_models_are_not_deletable() {
        assert!(!is_deletable(&record(SourceKind::builtin(), "")));
        assert!(!is_deletable(&record(SourceKind::endpoint(), "")));
        assert!(is_deletable(&record(SourceKind::ollama(), "")));
        assert!(is_deletable(&record(SourceKind::huggingface_cache(), "/x")));
    }

    #[test]
    fn not_deletable_messages_are_kind_specific() {
        assert!(not_deletable_message(&SourceKind::builtin()).contains("built-in"));
        assert!(not_deletable_message(&SourceKind::endpoint()).contains("connections"));
        assert!(not_deletable_message(&SourceKind::lm_studio()).contains("can't be deleted"));
    }

    #[test]
    fn an_ollama_model_previews_a_daemon_delete_with_no_paths() {
        let mut rec = record(SourceKind::ollama(), "");
        rec.footprint_mb = Some(2048);
        let preview = preview(&rec);
        assert!(preview.via_daemon);
        assert!(preview.paths.is_empty());
        assert_eq!(preview.bytes_estimate, 2048i64 << 20);
    }

    #[test]
    fn a_missing_folder_model_previews_no_paths_and_zero_bytes() {
        let mut rec = record(SourceKind::folder(), "/no/such/model/dir");
        rec.state = ModelState::Missing;
        let preview = preview(&rec);
        assert!(!preview.via_daemon);
        assert!(preview.paths.is_empty()); // the dir doesn't exist
        assert_eq!(preview.bytes_estimate, 0);
    }

    #[test]
    fn a_present_folder_model_lists_its_directory() {
        let dir =
            std::env::temp_dir().join(format!("hedos-removal-{:?}", std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rec = record(SourceKind::folder(), dir.to_str().unwrap());
        rec.footprint_mb = Some(10);
        let preview = preview(&rec);
        assert_eq!(preview.paths, vec![dir.to_string_lossy().into_owned()]);
        assert_eq!(preview.bytes_estimate, 10i64 << 20);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_shard_single_file_model_lists_just_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "hedos-removal-single-{:?}",
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("model.gguf");
        std::fs::write(&file, b"x").unwrap();
        let rec = record(SourceKind::file(), file.to_str().unwrap());
        let preview = preview(&rec);
        assert_eq!(preview.paths, vec![file.to_string_lossy().into_owned()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_sharded_file_model_groups_all_shards() {
        let dir = std::env::temp_dir().join(format!(
            "hedos-removal-shards-{:?}",
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "model-00001-of-00003.gguf",
            "model-00002-of-00003.gguf",
            "model-00003-of-00003.gguf",
            "unrelated.gguf",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let first = dir.join("model-00001-of-00003.gguf");
        let rec = record(SourceKind::file(), first.to_str().unwrap());
        let preview = preview(&rec);
        assert_eq!(preview.paths.len(), 3, "{:?}", preview.paths);
        assert!(preview.paths.iter().all(|p| p.contains("of-00003")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
