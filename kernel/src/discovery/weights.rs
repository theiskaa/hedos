//! One rule for reading GGUF weights and projectors, shared by every store
//! scanner (which records the file a server will load) and by identification
//! (which reads that file's header): what counts as a GGUF file and how big it
//! is, how a directory of them is walked, and which files make up a shard set.

use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::{is_gguf_name, is_mmproj_name};
use crate::discovery::gguf_shards::{ShardName, parse};

/// How far below a weights directory the walk looks. A hub snapshot keeps its
/// weights at the root, and a repo that ships one quantization per directory
/// keeps them a level down; the third level is slack for a repo that groups
/// those directories under one of its own.
const MAX_DEPTH: usize = 3;

/// What one GGUF file is, with its size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GgufFile {
    /// A file a model could be served from.
    Weight(u64),
    /// A multimodal projector, which lets the weights beside it see.
    Projector(u64),
}

/// What `path` is as a GGUF file: `None` unless its name is UTF-8 and reads as
/// a GGUF, and it resolves (following links) to a regular file. A link pointing
/// at a directory, or at nothing, is not a file and so is never taken for a
/// weight, whatever its name says.
pub(crate) fn gguf_file(path: &Path) -> Option<GgufFile> {
    let name = path.file_name()?.to_str()?;
    if !is_gguf_name(name) {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(match is_mmproj_name(name) {
        true => GgufFile::Projector(metadata.len()),
        false => GgufFile::Weight(metadata.len()),
    })
}

/// The size of `path` when it is a GGUF weight by [`gguf_file`]'s rule, and
/// `None` for a projector or anything that is not a GGUF file.
pub(crate) fn gguf_weight(path: &Path) -> Option<u64> {
    match gguf_file(path)? {
        GgufFile::Weight(bytes) => Some(bytes),
        GgufFile::Projector(_) => None,
    }
}

/// The GGUF files under one directory of weights.
#[derive(Debug, Default)]
pub(crate) struct GgufTree {
    /// The files a model could be served from, each with its size, sorted by
    /// path.
    pub weights: Vec<(PathBuf, u64)>,
    /// The multimodal projectors among them, each with its size, sorted by
    /// path.
    pub projectors: Vec<(PathBuf, u64)>,
}

/// Every GGUF file under `dir`, split into the weights a model is served from
/// and the projectors that accompany them.
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
    tree.projectors.sort();
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
        // A symlink lands here rather than in the branch above, and
        // `gguf_file` resolves it.
        match gguf_file(&path) {
            Some(GgufFile::Weight(bytes)) => tree.weights.push((path, bytes)),
            Some(GgufFile::Projector(bytes)) => tree.projectors.push((path, bytes)),
            None => {}
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
    if shard_of(path).is_none() {
        return Some(path.to_path_buf());
    }
    shard_set(path, files)
        .into_iter()
        .find(|(member, _)| shard_of(member).is_some_and(|shard| shard.index == 1))
        .map(|(first, _)| first.clone())
}

/// The entries of `files` that load together with `path`: when it is a shard,
/// every member of its set present beside it (same directory, base, and
/// declared total), and otherwise its own entry alone. Empty when `path` is not
/// a shard and has no entry in `files`.
///
/// Beside it, not merely under the same root: two quantization directories can
/// hold sets that share a base name, and the other set is not this one's.
pub(crate) fn shard_set<'a>(path: &Path, files: &'a [(PathBuf, u64)]) -> Vec<&'a (PathBuf, u64)> {
    let Some(shard) = shard_of(path) else {
        return files.iter().filter(|(file, _)| file == path).collect();
    };
    files
        .iter()
        .filter(|(candidate, _)| {
            candidate.parent() == path.parent()
                && shard_of(candidate)
                    .is_some_and(|member| member.total == shard.total && member.base == shard.base)
        })
        .collect()
}

fn shard_of(path: &Path) -> Option<ShardName> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "hedos-weights-{label}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, relative: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0u8; bytes]).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn gguf_file_tells_weights_projectors_and_impostors_apart() {
        let scratch = Scratch::new("kinds");
        let weight = scratch.file("x.GGUF", 7);
        let projector = scratch.file("x-mmproj-f16.gguf", 3);
        let partial = scratch.file("x.gguf.part", 5);
        let bin = scratch.file("x.bin", 5);
        let directory = scratch.0.join("d.gguf");
        std::fs::create_dir_all(&directory).unwrap();

        assert_eq!(gguf_file(&weight), Some(GgufFile::Weight(7)));
        assert_eq!(gguf_file(&projector), Some(GgufFile::Projector(3)));
        assert_eq!(gguf_weight(&weight), Some(7));
        assert_eq!(gguf_weight(&projector), None);
        for impostor in [&directory, &partial, &bin] {
            assert_eq!(gguf_file(impostor), None, "{}", impostor.display());
        }
    }

    #[test]
    fn a_tree_lists_its_projectors_with_their_sizes() {
        let scratch = Scratch::new("tree");
        let weight = scratch.file("model.Q4_K_M.gguf", 10);
        let f32 = scratch.file("mmproj-f32.gguf", 20);
        let f16 = scratch.file("Q8_0/mmproj-f16.gguf", 15);

        let tree = gguf_tree(&scratch.0);

        assert_eq!(tree.weights, vec![(weight, 10)]);
        let mut expected = vec![(f32, 20), (f16, 15)];
        expected.sort();
        assert_eq!(tree.projectors, expected);
    }

    #[test]
    fn shard_set_takes_the_members_beside_it_with_the_same_base_and_total() {
        let root = PathBuf::from("/repo");
        let files: Vec<(PathBuf, u64)> = vec![
            (root.join("Q4/m-00001-of-00002.gguf"), 1),
            (root.join("Q4/m-00002-of-00002.gguf"), 2),
            (root.join("Q4/m-00001-of-00003.gguf"), 3),
            (root.join("Q8/m-00001-of-00002.gguf"), 4),
            (root.join("Q8/m-00002-of-00002.gguf"), 5),
            (root.join("Q4/other.gguf"), 6),
        ];

        let set = shard_set(&root.join("Q4/m-00002-of-00002.gguf"), &files);
        assert_eq!(set, vec![&files[0], &files[1]]);

        let alone = shard_set(&root.join("Q4/other.gguf"), &files);
        assert_eq!(alone, vec![&files[5]]);
    }
}
