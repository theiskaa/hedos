//! One rule for reading the GGUF weights out of a directory, shared by the
//! store scanners (which record the file a server will load) and by
//! identification (which reads that file's header). The two used to walk the
//! same directories by different rules and could name different files.

use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::{is_gguf_name, is_mmproj_name};
use crate::discovery::gguf_shards::parse;

/// How far below a weights directory the walk looks. A hub snapshot keeps its
/// weights at the root, and a repo that ships one quantization per directory
/// keeps them a level down; the third level is slack for a repo that groups
/// those directories under one of its own.
const MAX_DEPTH: usize = 3;

/// The GGUF files under one directory of weights.
#[derive(Debug, Default)]
pub(crate) struct GgufTree {
    /// The files a model could be served from, each with its size, sorted by
    /// path.
    pub weights: Vec<(PathBuf, u64)>,
    /// Whether a multimodal projector sits among them, which is what lets the
    /// weights beside it see.
    pub has_projector: bool,
}

/// Every GGUF file under `dir`, split into the weights a model is served from
/// and whether a projector accompanies them.
///
/// Hidden entries are skipped: in the directories this reads they are
/// leftovers, not weights. Sizes are read through symlinks, because a hub
/// snapshot's entries are links into the blob store and the size that matters
/// is the target's; directories are recognized without following links, so a
/// link pointing back up its own tree cannot make the walk loop.
pub(crate) fn gguf_tree(dir: &Path) -> GgufTree {
    let mut tree = GgufTree::default();
    collect(dir, MAX_DEPTH, &mut tree);
    tree.weights.sort();
    tree
}

fn collect(dir: &Path, depth: usize, tree: &mut GgufTree) {
    if depth == 0 {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect(&path, depth - 1, tree);
            continue;
        }
        if !is_gguf_name(name) {
            continue;
        }
        let projector = is_mmproj_name(name);
        // A symlink lands here rather than in the branch above, and the
        // metadata resolves it: one pointing at a directory is not a file and
        // is dropped, so a directory wearing the name of a weight is never
        // taken for one.
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        match projector {
            true => tree.has_projector = true,
            false => tree.weights.push((path, metadata.len())),
        }
    }
}

/// The GGUF file a directory of weights is served from, out of the set
/// [`gguf_tree`] walked.
///
/// Candidates are weighed largest first, with a tie going to the earlier path
/// so that one directory always reads the same way, and the first that a server
/// could actually load wins. A sharded set can be loaded only through its first
/// file, found by index among the files beside it rather than by rebuilding the
/// name, so a set spelled `.GGUF` is answered like any other; a set missing that
/// file is passed over for the next candidate, and a directory holding nothing
/// but such a set answers with nothing.
pub(crate) fn primary_of(files: &[(PathBuf, u64)]) -> Option<PathBuf> {
    let mut candidates: Vec<&(PathBuf, u64)> = files.iter().collect();
    candidates.sort_by(|(left_path, left), (right_path, right)| {
        right.cmp(left).then_with(|| left_path.cmp(right_path))
    });
    candidates
        .into_iter()
        .find_map(|(path, _)| loadable(path, files))
}

/// The file a server would open to load `path`: `path` itself, or the first
/// file of the shard set it belongs to. `None` when it belongs to a set whose
/// first file is not there.
fn loadable(path: &Path, files: &[(PathBuf, u64)]) -> Option<PathBuf> {
    let Some(shard) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(parse)
    else {
        return Some(path.to_path_buf());
    };
    // Beside it, not merely under the same root: two quantization directories
    // can hold sets that share a base name, and the first file of the other set
    // is not this set's.
    files
        .iter()
        .find(|(candidate, _)| {
            candidate.parent() == path.parent()
                && candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(parse)
                    .is_some_and(|member| {
                        member.index == 1
                            && member.total == shard.total
                            && member.base == shard.base
                    })
        })
        .map(|(first, _)| first.clone())
}
