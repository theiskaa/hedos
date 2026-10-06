//! What serving a hub snapshot's primary weight loads, measured apart from
//! what the repo holds on disk. A repo can keep every quantization of a model,
//! and blobs from revisions it no longer points at; serving loads one weight
//! set, its projector, and the small files beside it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::is_gguf_name;
use crate::discovery::weights::{GgufTree, shard_set};
use crate::install::file_selection::is_support_file;
use crate::records::JsonValue;

/// The bytes serving `primary` loads out of `snapshot`, where `primary` is the
/// snapshot-side path of the weight a runtime opens (not the blob it links to)
/// and `tree` is the snapshot's GGUF walk.
///
/// It counts the primary's weight set (a GGUF's shard set, every shard a
/// safetensors index lists beside it, or the file alone), for a GGUF the
/// largest projector beside it or else at the snapshot root (a repo ships
/// several precisions of one and a runtime loads one), and the support files a
/// pull would fetch beside it that serving reads. Each file counts once,
/// however many snapshot entries link to it. `None` when none of that could be
/// measured, or when the weight set is missing a member, so a half-fetched
/// repo is never judged smaller than it will be.
pub(crate) fn serving_bytes(snapshot: &Path, primary: &Path, tree: &GgufTree) -> Option<i64> {
    let mut files: Vec<PathBuf> = weight_set(snapshot, primary, tree);
    if files.iter().any(|file| !file.is_file()) {
        return None;
    }
    if is_gguf(primary)
        && let Some(projector) = projector(snapshot, primary, tree)
    {
        files.push(projector);
    }
    files.extend(support_files(snapshot, primary));

    let mut sizes: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for file in files {
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let canonical = std::fs::canonicalize(&file).unwrap_or(file);
        sizes.insert(canonical, metadata.len());
    }
    if sizes.is_empty() {
        return None;
    }
    Some(
        sizes
            .values()
            .fold(0i64, |total, bytes| total.saturating_add(*bytes as i64)),
    )
}

/// Whether `name` is a safetensors index: `model.safetensors.index.json`, or
/// one naming a precision variant, `model.safetensors.index.fp16.json`.
pub(crate) fn is_safetensors_index(name: &str) -> bool {
    name.contains(".safetensors.index.") && name.ends_with(".json")
}

/// Whether `name` is the default safetensors index,
/// `model.safetensors.index.json`, rather than a precision variant's.
pub(crate) fn is_default_safetensors_index(name: &str) -> bool {
    name.ends_with(".safetensors.index.json")
}

/// The shard file names a safetensors index in `snapshot` maps its tensors
/// to, or `None` when the index cannot be read as one.
pub(crate) fn index_shards(snapshot: &Path, index_name: &str) -> Option<BTreeSet<String>> {
    let bytes = crate::fs::read_regular(&snapshot.join(index_name)).ok()?;
    let JsonValue::Object(json) = serde_json::from_slice::<JsonValue>(&bytes).ok()? else {
        return None;
    };
    let Some(JsonValue::Object(weight_map)) = json.get("weight_map") else {
        return None;
    };
    Some(
        weight_map
            .values()
            .filter_map(JsonValue::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

fn weight_set(snapshot: &Path, primary: &Path, tree: &GgufTree) -> Vec<PathBuf> {
    if is_gguf(primary) {
        let set = shard_set(primary, &tree.weights);
        if set.is_empty() {
            return vec![primary.to_path_buf()];
        }
        return set.into_iter().map(|(path, _)| path.clone()).collect();
    }
    if is_safetensors(primary)
        && let Some(shards) = indexed_set(snapshot, primary)
    {
        return shards.iter().map(|shard| snapshot.join(shard)).collect();
    }
    vec![primary.to_path_buf()]
}

/// The shards of the root safetensors index that lists `primary`, when one
/// does. A repo with several precisions has an index per variant, and the
/// one listing `primary` is its set. A `consolidated.safetensors` beside an
/// index of other shards is not in it, and so answers alone.
fn indexed_set(snapshot: &Path, primary: &Path) -> Option<BTreeSet<String>> {
    let name = primary.file_name()?.to_str()?;
    std::fs::read_dir(snapshot)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|index| is_safetensors_index(index))
        .filter_map(|index| index_shards(snapshot, &index))
        .find(|shards| shards.contains(name))
}

fn projector(snapshot: &Path, primary: &Path, tree: &GgufTree) -> Option<PathBuf> {
    let largest_in = |dir: Option<&Path>| {
        tree.projectors
            .iter()
            .filter(|(path, _)| path.parent() == dir)
            .max_by(|(left_path, left), (right_path, right)| {
                left.cmp(right).then_with(|| right_path.cmp(left_path))
            })
            .map(|(path, _)| path.clone())
    };
    largest_in(primary.parent()).or_else(|| largest_in(Some(snapshot)))
}

fn support_files(snapshot: &Path, primary: &Path) -> Vec<PathBuf> {
    let Some(dir) = primary.parent() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') || is_gguf_name(name) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(snapshot)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        if is_support_file(&relative, metadata.len() as i64) && read_to_serve(name) {
            files.push(path);
        }
    }
    files
}

/// Whether serving reads a support file named `name`. An importance matrix
/// (`.imatrix`, `imatrix.dat`) is what a quantization was made with, kept
/// beside the weights by a pull but never loaded to serve them. A variant's
/// safetensors index is a map of the weights, not something loaded, and is
/// left out as the default index already is.
fn read_to_serve(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(lower.ends_with(".imatrix")
        || (lower.contains("imatrix") && lower.ends_with(".dat"))
        || is_safetensors_index(name))
}

fn is_gguf(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_gguf_name)
}

fn is_safetensors(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("safetensors"))
}
