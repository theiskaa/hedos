//! Choosing which files of a Hugging Face repo to download. A repo lists many
//! siblings; this picks the model weights (preferring one GGUF quantization, or the
//! safetensors set, dropping duplicate/converted formats) plus the small companion
//! config/tokenizer files, and drops docs, images, and non-model directories.
//!
//! Ports Swift `Install/HuggingFace/HFFileSelection.swift`.

use std::collections::{BTreeMap, BTreeSet};

use crate::discovery::gguf_shards;
use crate::install::bytes::saturating_sum;

const WEIGHT_EXTENSIONS: [&str; 6] = ["safetensors", "gguf", "bin", "ckpt", "pt", "pth"];
const EXCLUDED_EXTENSIONS: [&str; 8] = ["md", "png", "jpg", "jpeg", "gif", "webp", "msgpack", "h5"];
const EXCLUDED_DIRECTORIES: [&str; 3] = ["onnx", "openvino", "coreml"];
/// Quantizations in descending preference; the first one present is chosen.
const QUANT_PREFERENCE: [&str; 6] = ["q4_k_m", "q4_0", "q5_k_m", "q6_k", "q8_0", "f16"];
/// Companion files kept alongside GGUF weights must be at most this big (10 MiB).
const COMPANION_CAP: i64 = 10 << 20;
/// Support files kept for a transformers model must be at most this big (100 MiB).
const CONFIG_CAP: i64 = 100 << 20;

/// One file listed in a Hugging Face repo, as returned by the hub API.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HFSibling {
    /// The file's path within the repo.
    pub rfilename: String,
    /// The file's size in bytes, if the listing reported it.
    pub bytes: Option<i64>,
    /// The LFS object's SHA-256 (`lfs.oid`), when the file is stored in LFS. The
    /// download path uses it as the content-addressed blob name and to verify the
    /// bytes; plain (non-LFS) files don't report one.
    pub sha256: Option<String>,
}

impl HFSibling {
    /// A sibling for `rfilename` with no known LFS hash.
    pub fn new(rfilename: impl Into<String>, bytes: Option<i64>) -> Self {
        Self {
            rfilename: rfilename.into(),
            bytes,
            sha256: None,
        }
    }

    /// This sibling with its LFS SHA-256 set.
    pub fn with_sha256(mut self, sha256: Option<String>) -> Self {
        self.sha256 = sha256;
        self
    }

    /// Whether this file is a model weight (by extension).
    pub fn is_weight(&self) -> bool {
        is_weight_path(&self.rfilename)
    }
}

/// Whether `path`'s extension marks it a model weight file.
pub fn is_weight_path(path: &str) -> bool {
    WEIGHT_EXTENSIONS.contains(&file_extension(path).as_str())
}

/// The lowercased extension of `path` (the part after the last `.`), or empty when
/// there is none or the only dot is a leading one (a dotfile has no extension).
pub fn file_extension(path: &str) -> String {
    match path.rfind('.') {
        Some(dot) if dot > 0 => path[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// Select the files to download from a repo's `siblings`: the eligible weights
/// (one GGUF quant group, or the safetensors/pytorch set) plus small companions.
pub fn select(siblings: &[HFSibling]) -> Vec<HFSibling> {
    let kept: Vec<HFSibling> = siblings
        .iter()
        .filter(|s| is_eligible(s))
        .cloned()
        .collect();
    let ggufs: Vec<HFSibling> = kept
        .iter()
        .filter(|s| s.rfilename.to_lowercase().ends_with(".gguf"))
        .cloned()
        .collect();
    if !ggufs.is_empty() {
        let others: Vec<HFSibling> = kept
            .iter()
            .filter(|s| !ggufs.contains(s))
            .cloned()
            .collect();
        return gguf_selection(&ggufs, &others);
    }
    if kept.iter().any(|s| s.rfilename == "model_index.json") {
        return diffusers_selection(&kept);
    }
    transformers_selection(&kept)
}

/// The path split into non-empty `/`-segments (Swift `split(separator:)` omits
/// empty subsequences).
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

/// Whether a sibling is a plausible model file: not hidden, not a readme, not an
/// excluded extension, not a flax/tf checkpoint, and not under an excluded dir.
fn is_eligible(sibling: &HFSibling) -> bool {
    let path = &sibling.rfilename;
    let segments = segments(path);
    let Some(filename) = segments.last().copied() else {
        return false;
    };
    if segments.iter().any(|segment| segment.starts_with('.')) {
        return false;
    }
    if filename.to_lowercase().starts_with("readme") {
        return false;
    }
    if EXCLUDED_EXTENSIONS.contains(&file_extension(filename).as_str()) {
        return false;
    }
    let stem = filename.to_lowercase();
    if stem.starts_with("flax_model") || stem.starts_with("tf_model") {
        return false;
    }
    if segments.len() > 1
        && let Some(first) = segments.first()
        && EXCLUDED_DIRECTORIES.contains(&first.to_lowercase().as_str())
    {
        return false;
    }
    true
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    directory: String,
    base: String,
    total: usize,
}

/// Choose one complete GGUF quant group (plus any mmproj projector and small
/// companions). Shards are grouped by `(directory, base, total)`; a sharded group
/// is only a candidate once every shard index is present.
fn gguf_selection(ggufs: &[HFSibling], others: &[HFSibling]) -> Vec<HFSibling> {
    let mut groups: BTreeMap<GroupKey, Vec<HFSibling>> = BTreeMap::new();
    let mut seen_indices: BTreeMap<GroupKey, BTreeSet<usize>> = BTreeMap::new();
    for sibling in ggufs {
        let segments = segments(&sibling.rfilename);
        let filename = segments.last().copied().unwrap_or("");
        let directory = segments[..segments.len().saturating_sub(1)].join("/");
        if let Some(shard) = gguf_shards::parse(filename) {
            let key = GroupKey {
                directory,
                base: shard.base,
                total: shard.total,
            };
            groups.entry(key.clone()).or_default().push(sibling.clone());
            seen_indices.entry(key).or_default().insert(shard.index);
        } else {
            // A non-sharded GGUF: its base is the name without the `.gguf` suffix
            // (5 trailing bytes, regardless of case).
            let stem = &filename[..filename.len() - ".gguf".len()];
            let key = GroupKey {
                directory,
                base: stem.to_owned(),
                total: 0,
            };
            groups.entry(key).or_default().push(sibling.clone());
        }
    }

    let mmproj: Vec<HFSibling> = ggufs
        .iter()
        .filter(|s| s.rfilename.to_lowercase().contains("mmproj"))
        .cloned()
        .collect();

    // `BTreeMap` iteration is already ordered by `(directory, base, total)`, so the
    // surviving candidate groups come out in the Swift-sorted order.
    let ordered: Vec<Vec<HFSibling>> = groups
        .iter()
        .filter(|(key, _)| {
            !key.base.to_lowercase().contains("mmproj")
                && (key.total == 0 || seen_indices.get(*key).map(BTreeSet::len) == Some(key.total))
        })
        .map(|(_, group)| group.clone())
        .collect();

    let chosen = pick_quant_group(&ordered);
    if chosen.is_empty() {
        return Vec::new();
    }
    let companions: Vec<HFSibling> = others
        .iter()
        .filter(|s| s.bytes.unwrap_or(0) <= COMPANION_CAP)
        .cloned()
        .collect();

    let mut result = chosen.clone();
    result.extend(mmproj.into_iter().filter(|s| !chosen.contains(s)));
    result.extend(companions);
    result
}

/// Pick the group matching the highest quant preference, else the smallest by total
/// bytes.
fn pick_quant_group(groups: &[Vec<HFSibling>]) -> Vec<HFSibling> {
    if groups.is_empty() {
        return Vec::new();
    }
    for token in QUANT_PREFERENCE {
        if let Some(matched) = groups
            .iter()
            .find(|group| group.iter().any(|s| matches_quant(&s.rfilename, token)))
        {
            return matched.clone();
        }
    }
    groups
        .iter()
        .min_by_key(|group| saturating_sum(group.iter().filter_map(|s| s.bytes)))
        .cloned()
        .unwrap_or_default()
}

/// Whether `rfilename` contains `token` as a whole quant word (bounded by non-quant
/// characters on both sides), so `q4_0` matches `model-q4_0.gguf` but not `xq4_0y`.
fn matches_quant(rfilename: &str, token: &str) -> bool {
    let name = rfilename.to_lowercase();
    let mut start = 0;
    while let Some(offset) = name[start..].find(token) {
        let at = start + offset;
        let end = at + token.len();
        let before_ok = match name[..at].chars().next_back() {
            None => true,
            Some(character) => !is_quant_character(character),
        };
        let after_ok = match name[end..].chars().next() {
            None => true,
            Some(character) => !is_quant_character(character),
        };
        if before_ok && after_ok {
            return true;
        }
        start = end;
    }
    false
}

fn is_quant_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Diffusers selection: drop root-level weights when a subtree carries them, drop a
/// `.bin`/`.ckpt`/… when a `.safetensors` twin exists, and drop `.fp16.`/`.non_ema.`
/// variants when the plain form is present.
fn diffusers_selection(kept: &[HFSibling]) -> Vec<HFSibling> {
    let paths: BTreeSet<&str> = kept.iter().map(|s| s.rfilename.as_str()).collect();
    let tree_has_weights = kept
        .iter()
        .any(|s| s.rfilename.contains('/') && s.is_weight());
    kept.iter()
        .filter(|sibling| {
            let path = &sibling.rfilename;
            if tree_has_weights && !path.contains('/') && sibling.is_weight() {
                return false;
            }
            let ext = file_extension(path);
            if ["bin", "ckpt", "pt", "pth"].contains(&ext.as_str()) {
                let stem = &path[..path.len() - (ext.len() + 1)];
                if paths.contains(format!("{stem}.safetensors").as_str()) {
                    return false;
                }
            }
            for variant in [".fp16.", ".non_ema."] {
                if path.contains(variant) && paths.contains(path.replace(variant, ".").as_str()) {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Transformers selection: the root safetensors set (or the pytorch `.bin` set when
/// none), plus small support files (config/tokenizer), excluding index sidecars.
fn transformers_selection(kept: &[HFSibling]) -> Vec<HFSibling> {
    let root: Vec<&HFSibling> = kept.iter().filter(|s| !s.rfilename.contains('/')).collect();
    let safetensors: Vec<&HFSibling> = root
        .iter()
        .copied()
        .filter(|s| {
            file_extension(&s.rfilename) == "safetensors"
                || s.rfilename.ends_with(".safetensors.index.json")
        })
        .collect();
    let has_safetensors_weight = safetensors
        .iter()
        .any(|s| file_extension(&s.rfilename) == "safetensors");
    let weights: Vec<HFSibling> = if has_safetensors_weight {
        safetensors.iter().copied().cloned().collect()
    } else {
        root.iter()
            .copied()
            .filter(|s| {
                s.rfilename.starts_with("pytorch_model")
                    && (file_extension(&s.rfilename) == "bin"
                        || s.rfilename.ends_with(".bin.index.json"))
            })
            .cloned()
            .collect()
    };
    let support: Vec<HFSibling> = root
        .iter()
        .copied()
        .filter(|s| {
            !s.is_weight()
                && !s.rfilename.ends_with(".index.json")
                && s.bytes.unwrap_or(0) <= CONFIG_CAP
        })
        .cloned()
        .collect();

    let mut result = weights;
    result.extend(support);
    result
}
