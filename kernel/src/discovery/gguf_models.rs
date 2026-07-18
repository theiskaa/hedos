//! Turning a flat list of GGUF files into [`DiscoveredModel`]s: loose files
//! become one model each, shard sets become a single model keyed by their shared
//! base (flagged as still-downloading when incomplete). Shared by the file-tree
//! scanners (LM Studio, loose files).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::discovery::gguf_shards::group;
use crate::discovery::modality_hints::gguf_hint;
use crate::discovery::scanner::DiscoveredModel;
use crate::records::{ModelSource, SourceKind};

/// Whether a filename looks like a multimodal projector (never the primary
/// weight). Case-insensitive `mmproj` substring.
pub fn is_mmproj_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("mmproj")
}

/// Build discovered models from `files` (each a GGUF path and its byte size).
/// `repo` derives the repository label from a file's path (e.g. a relative
/// `<org>/<model>` prefix), or `None`. Returns the models and any per-shard-set
/// issues (a shard set missing its first part is skipped, not emitted).
pub fn discovered_models(
    files: &[(PathBuf, i64)],
    kind: &SourceKind,
    repo: impl Fn(&Path) -> Option<String>,
) -> (Vec<DiscoveredModel>, Vec<String>) {
    let (groups, loose) = group(files);
    let mut bytes_by_path: HashMap<&Path, i64> = HashMap::new();
    for (path, bytes) in files {
        bytes_by_path.entry(path.as_path()).or_insert(*bytes);
    }
    let hint = gguf_hint();
    let mut discovered = Vec::new();
    let mut issues = Vec::new();

    for path in &loose {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        let mut model = DiscoveredModel::new(name, source(kind, path, &repo));
        apply_hint(&mut model, &hint);
        model.footprint_bytes = bytes_by_path.get(path.as_path()).copied().unwrap_or(0);
        model.primary_weight_path = Some(display(path));
        discovered.push(model);
    }

    for shard_group in groups {
        let Some(first) = shard_group.first_shard() else {
            issues.push(format!(
                "sharded model {} is missing its first part — skipped",
                shard_group.base
            ));
            continue;
        };
        let mut model = DiscoveredModel::new(shard_group.base.clone(), source(kind, first, &repo));
        apply_hint(&mut model, &hint);
        model.footprint_bytes = shard_group.footprint_bytes();
        model.primary_weight_path = Some(display(first));
        model.downloading = !shard_group.complete();
        discovered.push(model);
    }

    (discovered, issues)
}

fn source(kind: &SourceKind, path: &Path, repo: &impl Fn(&Path) -> Option<String>) -> ModelSource {
    let mut source = ModelSource::new(kind.clone(), &display(path));
    source.repo = repo(path);
    source
}

fn apply_hint(model: &mut DiscoveredModel, hint: &crate::discovery::modality_hints::Hint) {
    model.modality_hint = hint.modality.clone();
    model.capabilities_hint = hint.capabilities.clone();
    model.execution_hint = hint.execution;
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
