//! Where a model's bytes are on disk: the files its footprint counts, the
//! files removing it deletes, and the bytes a set of models takes with each
//! file counted once.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::discovery::duplicates::{Identity, entry_of, file_identity, passed_links};
use crate::discovery::ollama_scanner::{
    config_as_ollama_writes, manifest_as_ollama_writes, manifest_blobs, manifest_bookkeeping,
};
use crate::discovery::service::store_rank;
use crate::records::{ModelRecord, ModelState, SourceKind};
use crate::removal::removable_paths;

/// The files `record`'s footprint counts, listed as its store's scanner lists
/// them: an Ollama model's layer blobs, every file in a Hugging Face repo's
/// or a folder bundle's directory, and a GGUF's file or shard set. `None` for
/// a store that keeps no files (built-in, endpoint), or when none are where
/// the record says.
pub fn footprint_files(record: &ModelRecord) -> Option<Vec<PathBuf>> {
    let source = Path::new(&record.source.path);
    let kind = &record.source.kind;
    let files = if *kind == SourceKind::ollama() {
        manifest_blobs(source)?
    } else if *kind == SourceKind::huggingface_cache() || *kind == SourceKind::folder() {
        directory_contents(source)
    } else if *kind == SourceKind::lm_studio() || *kind == SourceKind::file() {
        removable_paths(record)
    } else {
        return None;
    };
    (!files.is_empty()).then_some(files)
}

/// Every file under the directory `dir` (a Hugging Face repo's or a folder
/// bundle's), and every symlink to one (a snapshot's links into `blobs/`),
/// as far as it can be read: everything `hedos rm` deletes with it.
pub(crate) fn directory_contents(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    // A directory that cannot be read leaves out what is under it, as the
    // scanner's own walk always has.
    let _ = everything_under(dir, &mut files);
    files
}

/// What removing a model deletes, as `hedos rm` removes it.
#[derive(Debug, Default)]
pub(crate) struct RemovalSet {
    /// The files that hold the model. The model can go only when another
    /// keeps each of them, or a copy of it.
    pub(crate) content: Vec<PathBuf>,
    /// Files deleted with it that hold no model content, each checked to be
    /// in the shape its writer gives it (see [`removal_set`]). They are freed
    /// with the model, and no other model needs to keep them.
    pub(crate) bookkeeping: Vec<PathBuf>,
}

/// Every file removing `record` deletes, as `hedos rm` removes it: an Ollama
/// model's manifest, config and layer blobs (the daemon deletes each blob no
/// other tag uses), everything under a Hugging Face repo's directory or a
/// folder bundle, and a GGUF's file or shard set. A symlink is listed as
/// itself; a directory reached through one is not entered, since removing the
/// link leaves what it points to. `None` when the files cannot all be listed,
/// and for a repo or bundle that is itself a symlink, or a repo whose
/// `blobs/` is one: removing it removes only the link, so it frees nothing.
///
/// A file is bookkeeping only in the shape its writer gives it, so nothing
/// a person put there passes for it: a ref under a repo's `refs/` is a
/// regular file of at most 64 bytes holding a 40-character hex commit hash, a
/// marker under `.no_exist/` is empty, an Ollama manifest or config blob is
/// the JSON the daemon writes with no field it does not write, a `.DS_Store`
/// starts with its magic and is at most [`DS_STORE_BYTES`], and a `._X`
/// AppleDouble file starts with its magic, is at most [`APPLE_DOUBLE_BYTES`]
/// and describes a file `X` removed with it. Any other file is content.
pub(crate) fn removal_set(record: &ModelRecord) -> Option<RemovalSet> {
    let source = Path::new(&record.source.path);
    let kind = &record.source.kind;
    let set = if *kind == SourceKind::ollama() {
        let mut set = RemovalSet {
            content: manifest_blobs(source)?,
            bookkeeping: Vec::new(),
        };
        for file in manifest_bookkeeping(source)? {
            let written = if file == source {
                manifest_as_ollama_writes(&file)
            } else {
                config_as_ollama_writes(&file)
            };
            if written {
                set.bookkeeping.push(file);
            } else {
                set.content.push(file);
            }
        }
        set
    } else if *kind == SourceKind::huggingface_cache() || *kind == SourceKind::folder() {
        let hub = *kind == SourceKind::huggingface_cache();
        if hub && is_symlink(&source.join("blobs")) {
            return None;
        }
        let files = directory_files(source)?;
        let removed: HashSet<&Path> = files.iter().map(PathBuf::as_path).collect();
        let (bookkeeping, content) = files.iter().cloned().partition(|file: &PathBuf| {
            os_metadata(file, &removed) || (hub && hub_bookkeeping(source, file))
        });
        RemovalSet {
            content,
            bookkeeping,
        }
    } else {
        RemovalSet {
            content: footprint_files(record)?,
            bookkeeping: Vec::new(),
        }
    };
    (!set.content.is_empty()).then_some(set)
}

/// Everything under `dir` as [`everything_under`] lists it, or `None` when
/// `dir` is a symlink or not a directory.
fn directory_files(dir: &Path) -> Option<Vec<PathBuf>> {
    if !std::fs::symlink_metadata(dir).ok()?.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    everything_under(dir, &mut files)?;
    Some(files)
}

/// The most a ref under a Hugging Face repo's `refs/` holds: the hub writes
/// a 40-character commit hash there, with no newline.
const REF_BYTES: u64 = 64;

/// The most a `.DS_Store` counted as Finder's own holds. Finder writes one of
/// a few KiB for a folder of a few files; anything larger is held as content.
const DS_STORE_BYTES: u64 = 64 << 10;

/// The most an AppleDouble `._` file counted as the system's own holds: the
/// attributes and resource fork of the file it describes.
const APPLE_DOUBLE_BYTES: u64 = 1 << 20;

/// The magic a Finder `.DS_Store` starts with: a version word of 1, then
/// `Bud1`.
const DS_STORE_MAGIC: &[u8] = b"\x00\x00\x00\x01Bud1";

/// The magic an AppleDouble `._` file starts with.
const APPLE_DOUBLE_MAGIC: &[u8] = b"\x00\x05\x16\x07";

/// Whether `file`, under the Hugging Face repo `repo`, is the hub's own
/// bookkeeping: a ref under `refs/` holding a commit hash, or an empty marker
/// under `.no_exist/` (a file the hub found missing).
fn hub_bookkeeping(repo: &Path, file: &Path) -> bool {
    let Ok(relative) = file.strip_prefix(repo) else {
        return false;
    };
    match relative.components().next() {
        Some(Component::Normal(first)) if first == "refs" => {
            small_regular(file, REF_BYTES).is_some_and(|bytes| is_commit_hash(&bytes))
        }
        Some(Component::Normal(first)) if first == ".no_exist" => {
            small_regular(file, 0).is_some_and(|bytes| bytes.is_empty())
        }
        _ => false,
    }
}

/// Whether `file` is metadata the system writes beside the files of a
/// folder: a Finder `.DS_Store`, or an AppleDouble `._X` file holding the
/// attributes of a file `X` that is in `removed` too, on a volume that cannot
/// store them. An orphan `._` file describes nothing removed with it, so it
/// is content.
fn os_metadata(file: &Path, removed: &HashSet<&Path>) -> bool {
    let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let (magic, limit) = if name == ".DS_Store" {
        (DS_STORE_MAGIC, DS_STORE_BYTES)
    } else if let Some(described) = name.strip_prefix("._") {
        if described.is_empty() || !removed.contains(file.with_file_name(described).as_path()) {
            return false;
        }
        (APPLE_DOUBLE_MAGIC, APPLE_DOUBLE_BYTES)
    } else {
        return false;
    };
    small_regular(file, limit).is_some_and(|bytes| bytes.starts_with(magic))
}

fn is_commit_hash(bytes: &[u8]) -> bool {
    let hash = bytes.trim_ascii();
    hash.len() == 40 && hash.iter().all(u8::is_ascii_hexdigit)
}

/// The bytes of `file` when it is a regular file, not a symlink, of at most
/// `limit` bytes.
fn small_regular(file: &Path, limit: u64) -> Option<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(file).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    crate::fs::open_regular(file)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink())
}

/// The directory entries removing `record` deletes outright, each spelled
/// with its directory resolved: a Hugging Face repo's or a folder bundle's
/// directory, a GGUF's file or shard set, an Ollama model's manifest, config
/// and layer blobs.
pub(crate) fn removal_roots(record: &ModelRecord) -> Vec<PathBuf> {
    let source = Path::new(&record.source.path);
    let paths = if record.source.kind == SourceKind::ollama() {
        let mut paths = manifest_bookkeeping(source).unwrap_or_default();
        paths.extend(manifest_blobs(source).unwrap_or_default());
        paths
    } else {
        removable_paths(record)
    };
    paths.iter().filter_map(|path| entry_of(path)).collect()
}

/// The directory entries `record` reaches a file through a symlink on, and
/// where each of its symlinks leads, spelled with directories resolved as
/// [`removal_roots`] spells them: its own path, its weights, every symlink in
/// its directory (its `blobs/`, a linked snapshot or subdirectory, a link to
/// a file), and every link on the way. What lies under `own`, the entries
/// its own removal deletes, is left out.
pub(crate) fn linked_reach(record: &ModelRecord, own: &[PathBuf]) -> Vec<PathBuf> {
    let source = PathBuf::from(&record.source.path);
    let kind = &record.source.kind;
    let mut paths = vec![source.clone()];
    paths.extend(record.primary_weight_path.as_deref().map(PathBuf::from));
    if *kind == SourceKind::huggingface_cache() || *kind == SourceKind::folder() {
        symlinks_under(&source, &mut paths, 0);
    } else if *kind == SourceKind::ollama() {
        paths.extend(manifest_bookkeeping(&source).unwrap_or_default());
        paths.extend(manifest_blobs(&source).unwrap_or_default());
    } else {
        paths.extend(removable_paths(record));
    }
    let mut reach: Vec<PathBuf> = paths
        .iter()
        .flat_map(|path| passed_links(path))
        .filter(|entry| !own.iter().any(|root| entry.starts_with(root)))
        .collect();
    reach.sort();
    reach.dedup();
    reach
}

/// How deep [`symlinks_under`] walks: far past any model's layout, and short
/// of a loop of directories that only a bind mount could make.
const SYMLINK_WALK_DEPTH: usize = 32;

/// Every symlink under `dir`, to a file or a directory, into `into`. A
/// directory reached through a symlink is not entered: the link is listed,
/// and where it leads is read from it.
fn symlinks_under(dir: &Path, into: &mut Vec<PathBuf>, depth: usize) {
    if depth > SYMLINK_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            into.push(entry.path());
        } else if kind.is_dir() {
            symlinks_under(&entry.path(), into, depth + 1);
        }
    }
}

/// The bytes on disk per store of `records`, each file counted once by device
/// and inode, as `hedos scan` counts them. A record whose weights are gone
/// holds none.
pub fn disk_by_store(records: &[ModelRecord]) -> BTreeMap<SourceKind, i64> {
    bytes_by_store(
        records
            .iter()
            .filter(|record| record.state != ModelState::Missing)
            .map(|record| {
                (
                    record.source.kind.clone(),
                    footprint_files(record),
                    record.size_on_disk().unwrap_or(0),
                )
            })
            .collect(),
    )
}

/// The bytes on disk per store of `models`, each a store, the files its
/// footprint counts (`None` when they could not be listed) and the footprint.
/// Each file counts once, by device and inode: the stores are walked in
/// [`crate::discovery::DiscoverySummary::stores`] order, so a file two stores
/// reach counts under the first. A model whose files could not be listed
/// counts its footprint.
pub(crate) fn bytes_by_store(
    mut models: Vec<(SourceKind, Option<Vec<PathBuf>>, i64)>,
) -> BTreeMap<SourceKind, i64> {
    models.sort_by(|(a, ..), (b, ..)| (store_rank(a), a).cmp(&(store_rank(b), b)));
    let mut counted: HashSet<Identity> = HashSet::new();
    let mut per_kind: BTreeMap<SourceKind, i64> = BTreeMap::new();
    for (kind, files, footprint) in models {
        let bytes = per_kind.entry(kind).or_default();
        let Some(files) = files else {
            *bytes = bytes.saturating_add(footprint);
            continue;
        };
        for file in files {
            let Ok(metadata) = std::fs::metadata(&file) else {
                continue;
            };
            if metadata.is_file() && counted.insert(file_identity(&file, &metadata)) {
                *bytes = bytes.saturating_add(metadata.len() as i64);
            }
        }
    }
    per_kind
}

/// Every regular file under `dir`, and every symlink to one. A directory
/// reached through a symlink is not entered. `None` when a directory cannot
/// be read. What it leaves out holds no bytes: a pipe, a socket, a dangling
/// symlink, and a symlink to a directory, whose removal leaves the directory.
fn everything_under(dir: &Path, into: &mut Vec<PathBuf>) -> Option<()> {
    for entry in std::fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        let kind = entry.file_type().ok()?;
        if kind.is_dir() {
            everything_under(&path, into)?;
        } else if kind.is_file()
            || (kind.is_symlink()
                && std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()))
        {
            into.push(path);
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_hash_is_forty_hex_characters_with_space_around_it_at_most() {
        assert!(is_commit_hash(b"0123456789abcdef0123456789abcdef01234567"));
        assert!(is_commit_hash(
            b"0123456789ABCDEF0123456789abcdef01234567\n"
        ));
        assert!(!is_commit_hash(b"main"));
        assert!(!is_commit_hash(b"0123456789abcdef0123456789abcdef0123456"));
        assert!(!is_commit_hash(b"0123456789abcdef0123456789abcdef0123456g"));
    }
}
