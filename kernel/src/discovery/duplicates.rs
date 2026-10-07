//! Detecting duplicate model weights: a model can go in favour of another
//! when every file removing it deletes is kept by the other, as that same
//! file or as a copy matching it in size and in content sampled across the
//! file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::Metadata;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::discovery::service::store_rank;
use crate::records::SourceKind;

/// The default minimum (256 MiB) of the bytes removing a copy must free for
/// [`detect`] to offer it; smaller savings are not worth reporting.
pub const DEFAULT_THRESHOLD: i64 = 256 << 20;

const SAMPLE_SIZE: u64 = 1 << 20;

/// The region at the start of each file [`detect`] reads whole: a GGUF or
/// safetensors header, tokenizer included, sits there.
const HEAD_BYTES: u64 = 8 << 20;

/// The size of each block [`detect`] reads past the head.
const BLOCK_BYTES: u64 = 64 << 10;

/// How many blocks [`detect`] reads past the head, spread evenly to the end of
/// the file.
const BLOCKS: u64 = 256;

/// One model as duplicate detection names it: the record, and the file its
/// weights are reached through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateMember {
    /// The model's record id.
    pub id: String,
    /// The name the shelf shows for the model (its alias, when it has one).
    pub name: String,
    /// The store the model came from.
    pub kind: SourceKind,
    /// The primary weight file, as the model's record names it, or where
    /// the model is (its manifest, its directory) when it names none.
    pub path: String,
}

/// A model offered to [`detect`]: who it is, and every file removing it
/// deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCandidate {
    /// The model.
    pub member: DuplicateMember,
    /// Every file holding the model that removing it deletes, as `hedos rm`
    /// removes it: its weights and the files beside them (everything in a
    /// Hugging Face repo's or a folder bundle's directory, an Ollama model's
    /// layers). A symlink is listed as itself. The model can go only when
    /// another keeps each of these, or a copy of it.
    pub files: Vec<String>,
    /// The files removing the model also deletes that hold no model content:
    /// a Hugging Face repo's refs and empty `.no_exist` markers, an Ollama
    /// manifest and its config blob, Finder's `.DS_Store`. They are freed
    /// with the model, and no other model needs to keep them.
    pub bookkeeping: Vec<String>,
    /// The ids of the other models on the shelf that removing this one would
    /// break: one that reaches a file or directory its removal deletes
    /// through a symlink (a repo linked in from another disk, a linked
    /// snapshot or subdirectory), and one that is no candidate itself whose
    /// files its removal deletes. The model is never offered unless each of
    /// them goes with it, as a name of the same copy. Any other name here
    /// (a watched folder its removal deletes) keeps it from being offered.
    pub pinned_by: Vec<String>,
}

/// One physical set of files, and every model that removes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCopy {
    /// The model the copy is named by: one whose path is not a symlink, when
    /// any model reaches the copy through such a path.
    pub member: DuplicateMember,
    /// Every other model whose removal deletes the same files: through a
    /// symlink, a hard link, a second Ollama tag over the same blobs, or the
    /// same directory under another spelling. Removing the member and every
    /// one of these frees the space; removing one alone can free nothing
    /// while another keeps the files, and removing a symlink frees nothing.
    pub aliases: Vec<DuplicateMember>,
    /// The file that holds the bytes, when every file of the copy is reached
    /// only through a symlink, so removing it frees nothing.
    pub link_target: Option<String>,
}

/// A copy [`detect`] offers for removal, and what removing it frees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovableCopy {
    /// The copy.
    pub copy: DuplicateCopy,
    /// The bytes removing this copy alone (its member and every alias) frees:
    /// each of its files that it names directly, through every hard link the
    /// file has, and that no other model reaches. A file another model still
    /// reaches, or that a hard link elsewhere keeps, frees nothing.
    pub reclaimable_bytes: i64,
}

/// A copy to keep, and the copies that can go while it stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// The copy to keep.
    pub kept: DuplicateCopy,
    /// Every copy that can go in favour of `kept`, sorted by its member's
    /// name then path. Each file removing one deletes is a file `kept` keeps,
    /// or a copy of one, so removing all of them loses nothing.
    pub removable: Vec<RemovableCopy>,
    /// What removing every copy in `removable` frees, each file counted
    /// once: the sum of theirs, and more when two of them share a file (a
    /// hard link between them, or one Ollama blob), which goes only with the
    /// last of them.
    pub reclaimable_bytes: i64,
}

/// Find the models among `candidates` that can be removed because another
/// keeps everything they hold.
///
/// Candidates whose files are the same files (one device and inode for each,
/// so a symlink, a hard link and an Ollama blob two tags share all count) are
/// one copy. A copy can go in favour of another when every file removing it
/// deletes is either a file the other keeps, which frees nothing and loses
/// nothing, or a copy of one: the same size and the same content where it is
/// sampled. Bookkeeping files ([`DuplicateCandidate::bookkeeping`]) need no
/// counterpart. The rule is directional: an Ollama model beside an LM Studio
/// clone of its weights keeps a template the clone lacks, so the clone can go
/// and the Ollama model cannot (unless it has no template or parameters
/// layer). A copy keeps every file it reaches, directly or through a symlink,
/// since a copy whose removal would take away an entry on the way is never
/// offered.
///
/// A copy is offered only when removing it frees at least `threshold` bytes
/// (and at least one): the files it names directly, through every directory
/// entry each has (`nlink`, against the distinct entries its names spell),
/// that no other candidate names. It is never offered when removing it would
/// take away a directory entry another candidate lists, or passes through on
/// a symlink, since that candidate would lose its file; a blob two Ollama
/// models list is the exception, as the daemon keeps it for the other. Nor
/// is it offered when a model outside its copy pins it
/// ([`DuplicateCandidate::pinned_by`]). A
/// candidate one of whose files cannot be read as a regular file, or whose
/// directory cannot be resolved, is left out.
///
/// The copies that can go are gathered around the ones kept, once content is
/// compared. A copy that cannot go itself is kept first, then one with a file
/// it names other than through a symlink; among the rest, a copy with an
/// Ollama name, then the Hugging Face cache, then LM Studio, is kept before a
/// loose file, then the copy whose member sorts first by name and path. Each
/// copy offered goes in favour of a kept one, and no kept copy is offered, so
/// removing every copy offered loses no data. Copies offered for each other
/// in both directions (two identical loose files) are one group with one
/// kept.
///
/// Sampling reads, of each file, the first 8 MiB and then 256 blocks of
/// 64 KiB spread evenly over the rest to its last byte, or the whole file when
/// it is smaller than that: at most 24 MiB per file, and only for files whose
/// size another file shares. The heads are compared first, and two files stop
/// being read at the first sample where they differ. A difference wider than
/// the step between two blocks (under 1 MiB for a 256 MiB file, about 4 MiB
/// more for each GiB) is always seen, as a fine-tune's weights, changed
/// throughout, are; a narrower one can be missed, so a copy is identical in
/// size and sampled content, not proven byte for byte. Each block is a seek,
/// so files that agree all the way cost up to 257 reads each, which a
/// spinning or network disk feels. Every file is also stat-ed and its
/// directory resolved once, and a folder bundle or repo is listed in full
/// (a cloned model's `.git` included) to build the candidates.
///
/// Groups are returned most-reclaimable first, then by the name of the copy
/// kept.
pub fn detect(candidates: &[DuplicateCandidate], threshold: i64) -> Vec<DuplicateGroup> {
    find(candidates, threshold).0
}

/// [`detect`]'s groups, and what removing every copy they offer frees, each
/// file counted once.
pub(crate) fn find(
    candidates: &[DuplicateCandidate],
    threshold: i64,
) -> (Vec<DuplicateGroup>, i64) {
    let copies = copies_of(candidates);
    let shared = Sharing::over(&copies);
    let mut traits: Vec<Traits> = (0..copies.len())
        .map(|index| shared.traits(index, &copies))
        .collect();
    for item in &mut traits {
        item.offerable &= item.frees > 0 && item.frees >= threshold;
    }

    let mut keeping: HashMap<&Identity, Vec<usize>> = HashMap::new();
    let mut kept_by_size: HashMap<u64, BTreeSet<&Identity>> = HashMap::new();
    for (index, item) in traits.iter().enumerate() {
        for &(identity, size) in &item.keeps {
            keeping.entry(identity).or_default().push(index);
            kept_by_size.entry(size).or_default().insert(identity);
        }
    }

    let mut possible: Vec<Vec<usize>> = vec![Vec::new(); copies.len()];
    for (index, copy) in copies.iter().enumerate() {
        if traits[index].offerable {
            possible[index] = keepers_by_size(index, copy, &kept_by_size, &keeping);
            traits[index].offerable = !possible[index].is_empty();
        }
    }

    let classes = content_classes(&copies, &traits, &kept_by_size);
    let kept_classes: Vec<HashSet<usize>> = traits
        .iter()
        .map(|item| {
            item.keeps
                .iter()
                .filter_map(|(identity, _)| classes.get(identity).copied())
                .collect()
        })
        .collect();
    let favours: Vec<Vec<usize>> = copies
        .iter()
        .enumerate()
        .map(|(index, copy)| {
            possible[index]
                .iter()
                .copied()
                .filter(|&other| {
                    copy.content().all(|(identity, _)| {
                        classes
                            .get(identity)
                            .is_some_and(|class| kept_classes[other].contains(class))
                    })
                })
                .collect()
        })
        .collect();
    // Whether a copy can go is known only now: one whose sizes matched but
    // whose content did not is kept like any other that cannot go.
    for (item, keepers) in traits.iter_mut().zip(&favours) {
        item.offerable = !keepers.is_empty();
    }

    let mut offered_anywhere: BTreeSet<usize> = BTreeSet::new();
    let mut groups: Vec<DuplicateGroup> = gather(&copies, &traits, &favours)
        .into_iter()
        .filter_map(|(keeper, offered)| {
            let kept = copies[keeper].named()?;
            let mut removable: Vec<RemovableCopy> = offered
                .iter()
                .filter_map(|&index| {
                    Some(RemovableCopy {
                        copy: copies[index].named()?,
                        reclaimable_bytes: traits[index].frees,
                    })
                })
                .collect();
            if removable.is_empty() {
                return None;
            }
            removable.sort_by(|a, b| order_of(&a.copy).cmp(&order_of(&b.copy)));
            offered_anywhere.extend(&offered);
            Some(DuplicateGroup {
                kept,
                removable,
                reclaimable_bytes: shared.frees(&offered, &copies),
            })
        })
        .collect();
    groups.sort_by(|a, b| {
        b.reclaimable_bytes
            .cmp(&a.reclaimable_bytes)
            .then_with(|| order_of(&a.kept).cmp(&order_of(&b.kept)))
    });
    let total = shared.frees(&offered_anywhere, &copies);
    (groups, total)
}

/// One physical copy while it is being found: its files by identity, and
/// every candidate reaching them with whether that candidate's path is a
/// symlink.
struct Found<'a> {
    files: BTreeMap<Identity, StoredFile>,
    names: Vec<(&'a DuplicateCandidate, bool)>,
}

impl<'a> Found<'a> {
    fn add(
        &mut self,
        candidate: &'a DuplicateCandidate,
        linked: bool,
        files: BTreeMap<Identity, StoredFile>,
    ) {
        for (identity, file) in files {
            match self.files.get_mut(&identity) {
                Some(stored) => {
                    stored.direct.extend(file.direct);
                    stored.linked.extend(file.linked);
                    stored.bookkeeping &= file.bookkeeping;
                }
                None => {
                    self.files.insert(identity, file);
                }
            }
        }
        self.names.push((candidate, linked));
    }

    /// The files that hold the model, without the bookkeeping.
    fn content(&self) -> impl Iterator<Item = (&Identity, &StoredFile)> {
        self.files.iter().filter(|(_, file)| !file.bookkeeping)
    }

    /// Whether every model removing the copy is an Ollama model, whose blobs
    /// the daemon deletes only once no other Ollama model lists them.
    fn only_ollama(&self) -> bool {
        self.names
            .iter()
            .all(|(candidate, _)| candidate.member.kind == SourceKind::ollama())
    }

    /// Whether a model outside the copy would break were it removed.
    fn pinned(&self) -> bool {
        self.names.iter().any(|(candidate, _)| {
            candidate
                .pinned_by
                .iter()
                .any(|id| !self.names.iter().any(|(name, _)| name.member.id == *id))
        })
    }

    /// Whether every file holding the copy is reached only through a
    /// symlink, so removing it frees none of them.
    fn only_through_links(&self) -> bool {
        self.content().all(|(_, file)| file.direct.is_empty())
    }

    /// The best place among the stores of the copy's names: an Ollama name
    /// ranks the copy as Ollama, whatever name it is reported by.
    fn best_store(&self) -> usize {
        self.names
            .iter()
            .map(|(candidate, _)| store_rank(&candidate.member.kind))
            .min()
            .unwrap_or(usize::MAX)
    }

    /// The candidate the copy is reported by: one whose path is not a link
    /// when there is one, then the first by name and path.
    fn first_name(&self) -> Option<&'a DuplicateMember> {
        self.names
            .iter()
            .map(|(candidate, linked)| (&candidate.member, *linked))
            .min_by(|(a, a_linked), (b, b_linked)| (a_linked, order(a)).cmp(&(b_linked, order(b))))
            .map(|(member, _)| member)
    }

    /// The copy as reported: named by [`Found::first_name`], the other
    /// candidates kept as its aliases.
    fn named(&self) -> Option<DuplicateCopy> {
        let member = self.first_name()?;
        let link_target = (self.only_through_links() && is_symlink(Path::new(&member.path)))
            .then(|| std::fs::canonicalize(&member.path).ok())
            .flatten()
            .map(|path| path.to_string_lossy().into_owned());
        let mut aliases: Vec<DuplicateMember> = self
            .names
            .iter()
            .map(|(candidate, _)| &candidate.member)
            .filter(|alias| !std::ptr::eq(*alias, member))
            .cloned()
            .collect();
        aliases.sort_by(|a, b| order(a).cmp(&order(b)));
        Some(DuplicateCopy {
            member: member.clone(),
            aliases,
            link_target,
        })
    }
}

/// The candidates as physical copies: those whose files holding the model
/// are the same files are one copy.
fn copies_of(candidates: &[DuplicateCandidate]) -> Vec<Found<'_>> {
    let mut copies: Vec<Found> = Vec::new();
    let mut by_identity: HashMap<Vec<Identity>, usize> = HashMap::new();
    for candidate in candidates {
        let Some(files) = files_of(candidate) else {
            continue;
        };
        let identities: Vec<Identity> = files
            .iter()
            .filter(|(_, file)| !file.bookkeeping)
            .map(|(identity, _)| identity)
            .cloned()
            .collect();
        let linked = is_symlink(Path::new(&candidate.member.path));
        match by_identity.get(&identities) {
            Some(&index) => copies[index].add(candidate, linked, files),
            None => {
                by_identity.insert(identities, copies.len());
                let mut found = Found {
                    files: BTreeMap::new(),
                    names: Vec::new(),
                };
                found.add(candidate, linked, files);
                copies.push(found);
            }
        }
    }
    copies
}

/// How the copies share their files and the directory entries that reach
/// them.
struct Sharing<'c> {
    /// The copies naming each file directly.
    namers: HashMap<&'c Identity, BTreeSet<usize>>,
    /// The directory entries naming each file directly, across every copy.
    named_entries: HashMap<&'c Identity, BTreeSet<&'c Path>>,
    /// The copies listing each directory entry, a file or a symlink.
    listing: HashMap<&'c Path, BTreeSet<usize>>,
    /// The copies whose symlinks pass through each directory entry.
    leaning: HashMap<PathBuf, BTreeSet<usize>>,
}

/// What [`detect`] knows of one copy before comparing content.
struct Traits<'c> {
    /// The bytes removing it alone frees.
    frees: i64,
    /// Whether it may be offered: it frees enough, and removing it takes
    /// away no directory entry another copy lists or links through. Once
    /// content is compared, whether it can go in favour of another.
    offerable: bool,
    /// Every file holding the model it reaches, with its size. Each stays
    /// while the copy does: one it names directly by that name, one it
    /// reaches through a symlink because no copy removing the entries on the
    /// way is ever offered.
    keeps: Vec<(&'c Identity, u64)>,
}

impl<'c> Sharing<'c> {
    fn over(copies: &'c [Found]) -> Self {
        let mut namers: HashMap<&Identity, BTreeSet<usize>> = HashMap::new();
        let mut named_entries: HashMap<&Identity, BTreeSet<&Path>> = HashMap::new();
        let mut listing: HashMap<&Path, BTreeSet<usize>> = HashMap::new();
        let mut leaning: HashMap<PathBuf, BTreeSet<usize>> = HashMap::new();
        for (index, copy) in copies.iter().enumerate() {
            for (identity, file) in &copy.files {
                for entry in &file.direct {
                    namers.entry(identity).or_default().insert(index);
                    named_entries
                        .entry(identity)
                        .or_default()
                        .insert(entry.as_path());
                }
                for entry in file.direct.iter().chain(&file.linked) {
                    listing.entry(entry.as_path()).or_default().insert(index);
                }
                for entry in &file.linked {
                    for passed in passed_links(entry) {
                        leaning.entry(passed).or_default().insert(index);
                    }
                }
            }
        }
        Self {
            namers,
            named_entries,
            listing,
            leaning,
        }
    }

    /// The bytes removing every copy in `removed` frees, each file once: a
    /// file goes when every copy naming it directly is removed and those
    /// names are all of its directory entries. A blob two Ollama models list
    /// is deleted only with the last of them, and a file a hard link outside
    /// the copies keeps is never freed.
    fn frees(&self, removed: &BTreeSet<usize>, copies: &[Found]) -> i64 {
        let mut counted: HashSet<&Identity> = HashSet::new();
        let mut bytes = 0i64;
        for &index in removed {
            for (identity, file) in &copies[index].files {
                if !counted.insert(identity) {
                    continue;
                }
                let all_named = self
                    .namers
                    .get(identity)
                    .is_some_and(|namers| namers.is_subset(removed));
                let entries = self.named_entries.get(identity).map_or(0, BTreeSet::len);
                if all_named && entries as u64 >= file.links {
                    bytes = bytes.saturating_add(i64::try_from(file.size).unwrap_or(i64::MAX));
                }
            }
        }
        bytes
    }

    fn traits(&self, index: usize, copies: &'c [Found]) -> Traits<'c> {
        let copy = &copies[index];
        let mut offerable = !copy.pinned();
        let mut keeps = Vec::new();
        for (identity, file) in &copy.files {
            for entry in file.direct.iter().chain(&file.linked) {
                let blob = file.direct.contains(entry) && copy.only_ollama();
                let listed_elsewhere = others(&self.listing, entry, index)
                    .any(|other| !(blob && copies[other].only_ollama()));
                let leaned_on = others(&self.leaning, entry, index).next().is_some();
                if listed_elsewhere || leaned_on {
                    offerable = false;
                }
            }
            if !file.bookkeeping {
                keeps.push((identity, file.size));
            }
        }
        Traits {
            frees: self.frees(&BTreeSet::from([index]), copies),
            offerable,
            keeps,
        }
    }
}

/// The copies other than `index` that `copies_at` holds for `entry`.
fn others<'m, K>(
    copies_at: &'m HashMap<K, BTreeSet<usize>>,
    entry: &Path,
    index: usize,
) -> impl Iterator<Item = usize> + 'm
where
    K: std::borrow::Borrow<Path> + std::hash::Hash + Eq,
{
    copies_at
        .get(entry)
        .into_iter()
        .flatten()
        .copied()
        .filter(move |&other| other != index)
}

/// `path` with its directory resolved, so two spellings of one directory
/// entry compare equal. `None` when the directory cannot be resolved.
pub(crate) fn entry_of(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Some(std::fs::canonicalize(parent).ok()?.join(name))
}

/// A symlink `entry` with its own name as its directory lists it, found by
/// the link's inode, so a link reached by another case or Unicode form of its
/// name compares equal to the entry removal deletes. `entry` as given when the
/// directory cannot be read or lists no such link.
#[cfg(unix)]
fn spelled_on_disk(entry: &Path) -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let (Some(parent), Ok(link)) = (entry.parent(), std::fs::symlink_metadata(entry)) else {
        return entry.to_path_buf();
    };
    let Ok(listing) = std::fs::read_dir(parent) else {
        return entry.to_path_buf();
    };
    listing
        .flatten()
        .find(|listed| {
            listed
                .metadata()
                .is_ok_and(|seen| seen.dev() == link.dev() && seen.ino() == link.ino())
        })
        .map_or_else(|| entry.to_path_buf(), |listed| listed.path())
}

/// `entry` as given, where there is no inode to find the listed name by.
#[cfg(not(unix))]
fn spelled_on_disk(entry: &Path) -> PathBuf {
    entry.to_path_buf()
}

/// The most symlinks [`passed_links`] follows, as the system does before it
/// gives up on a loop.
const MAX_LINKS: usize = 40;

/// Every symlink `path` passes through, its own name and any directory on the
/// way included, each spelled with its directory resolved as [`entry_of`]
/// spells it, then where it ends, resolved in full. Resolved a component at a
/// time, as the system resolves a path; a part that does not exist is taken
/// as written. A link typed in another case or Unicode form than the disk's
/// still names the entries removal deletes, as those are spelled from the
/// disk.
pub(crate) fn passed_links(path: &Path) -> Vec<PathBuf> {
    let mut passed = Vec::new();
    let Ok(start) = std::path::absolute(path) else {
        return passed;
    };
    let mut resolved = PathBuf::new();
    let mut pending: Vec<std::ffi::OsString> = Vec::new();
    plan(&start, &mut resolved, &mut pending);
    let mut links = 0;
    while let Some(name) = pending.pop() {
        if name == ".." {
            resolved.pop();
            continue;
        }
        let next = resolved.join(&name);
        if !is_symlink(&next) {
            resolved = next;
            continue;
        }
        links += 1;
        let target = std::fs::read_link(&next);
        passed.push(entry_of(&next).map_or(next, |entry| spelled_on_disk(&entry)));
        let Ok(target) = target else {
            return passed;
        };
        if links >= MAX_LINKS {
            return passed;
        }
        plan(&target, &mut resolved, &mut pending);
    }
    let end = std::fs::canonicalize(&resolved)
        .ok()
        .or_else(|| entry_of(&resolved))
        .unwrap_or(resolved);
    passed.push(end);
    passed
}

/// Queue the components of `path` to resolve next, starting again from its
/// root when it has one.
fn plan(path: &Path, resolved: &mut PathBuf, pending: &mut Vec<std::ffi::OsString>) {
    if path.has_root() {
        *resolved = PathBuf::new();
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => parts.push(std::ffi::OsString::from("..")),
            Component::Normal(name) => parts.push(name.to_owned()),
        }
    }
    pending.extend(parts.into_iter().rev());
}

/// The copies other than `index` that keep, for every file of `copy`, that
/// file or another of its size: the only ones it could go in favour of.
fn keepers_by_size(
    index: usize,
    copy: &Found,
    kept_by_size: &HashMap<u64, BTreeSet<&Identity>>,
    keeping: &HashMap<&Identity, Vec<usize>>,
) -> Vec<usize> {
    let mut possible: Option<BTreeSet<usize>> = None;
    for (_, file) in copy.content() {
        let here: BTreeSet<usize> = kept_by_size
            .get(&file.size)
            .into_iter()
            .flatten()
            .flat_map(|identity| keeping.get(identity).into_iter().flatten().copied())
            .filter(|&other| other != index)
            .collect();
        let narrowed: BTreeSet<usize> = match possible {
            Some(possible) => possible.intersection(&here).copied().collect(),
            None => here,
        };
        if narrowed.is_empty() {
            return Vec::new();
        }
        possible = Some(narrowed);
    }
    possible.into_iter().flatten().collect()
}

/// A class number for every file of every copy: files of the same size and
/// sampled content share one, and every other file has its own. Only the
/// sizes a copy that may be offered holds are read, against every file kept
/// at that size.
fn content_classes<'c>(
    copies: &'c [Found],
    traits: &[Traits<'c>],
    kept_by_size: &HashMap<u64, BTreeSet<&'c Identity>>,
) -> HashMap<&'c Identity, usize> {
    let mut classes: HashMap<&Identity, usize> = HashMap::new();
    let mut stored: HashMap<&Identity, &StoredFile> = HashMap::new();
    for copy in copies {
        for (identity, file) in &copy.files {
            let next = classes.len();
            classes.entry(identity).or_insert(next);
            stored.entry(identity).or_insert(file);
        }
    }

    let mut buckets: BTreeMap<u64, BTreeSet<&Identity>> = BTreeMap::new();
    for (copy, item) in copies.iter().zip(traits) {
        if !item.offerable {
            continue;
        }
        for (identity, file) in copy.content() {
            buckets.entry(file.size).or_default().insert(identity);
        }
    }
    for (size, bucket) in &mut buckets {
        bucket.extend(kept_by_size.get(size).into_iter().flatten().copied());
    }
    for bucket in buckets.into_values() {
        if bucket.len() < 2 {
            continue;
        }
        let files: Vec<(&Identity, &StoredFile)> = bucket
            .into_iter()
            .filter_map(|identity| stored.get(identity).map(|file| (identity, *file)))
            .collect();
        for class in identical(&files) {
            let Some(&number) = class.first().and_then(|first| classes.get(first)) else {
                continue;
            };
            for identity in class {
                classes.insert(identity, number);
            }
        }
    }
    classes
}

/// Gather the copies offered around the copies kept: the copies that cannot
/// go are kept first, then the others in [`keep_order`]; a copy is offered
/// in favour of the first kept copy it can go for. Each keeper comes with
/// the copies offered in its favour.
fn gather(
    copies: &[Found],
    traits: &[Traits],
    favours: &[Vec<usize>],
) -> Vec<(usize, BTreeSet<usize>)> {
    let mut order: Vec<usize> = (0..copies.len())
        .filter(|&index| {
            !favours[index].is_empty() || favours.iter().any(|keepers| keepers.contains(&index))
        })
        .collect();
    order.sort_by(|&a, &b| {
        keep_order(&copies[a], &traits[a]).cmp(&keep_order(&copies[b], &traits[b]))
    });

    let mut kept: Vec<usize> = Vec::new();
    let mut offered: BTreeMap<usize, usize> = BTreeMap::new();
    loop {
        for &index in &order {
            if kept.contains(&index) || offered.contains_key(&index) {
                continue;
            }
            if let Some(&keeper) = order
                .iter()
                .find(|keeper| kept.contains(keeper) && favours[index].contains(keeper))
            {
                offered.insert(index, keeper);
            }
        }
        let open = |index: &usize| !kept.contains(index) && !offered.contains_key(index);
        let next = order.iter().copied().find(|&keeper| {
            open(&keeper)
                && order
                    .iter()
                    .any(|other| open(other) && favours[*other].contains(&keeper))
        });
        match next {
            Some(keeper) => kept.push(keeper),
            None => break,
        }
    }

    kept.into_iter()
        .map(|keeper| {
            let favoured: BTreeSet<usize> = offered
                .iter()
                .filter(|&(_, &favoured)| favoured == keeper)
                .map(|(&index, _)| index)
                .collect();
            (keeper, favoured)
        })
        .collect()
}

/// Which copy to keep first: one that cannot go, then one with a file it
/// names other than through a symlink, then by the best store among its
/// names (Ollama, the Hugging Face cache and LM Studio before a loose file,
/// as the stores are listed), then by the name and path it is reported by.
fn keep_order<'a>(
    copy: &Found<'a>,
    traits: &Traits,
) -> (bool, bool, usize, Option<(&'a str, &'a str)>) {
    (
        traits.offerable,
        copy.only_through_links(),
        copy.best_store(),
        copy.first_name().map(order),
    )
}

fn order(member: &DuplicateMember) -> (&str, &str) {
    (&member.name, &member.path)
}

fn order_of(copy: &DuplicateCopy) -> (&str, &str) {
    order(&copy.member)
}

/// One file of a copy.
struct StoredFile {
    /// A path that reads it.
    path: PathBuf,
    size: u64,
    /// How many directory entries the file has.
    links: u64,
    /// The directory entries that name it directly, not through a symlink,
    /// each spelled with its directory resolved, so two spellings of one
    /// entry are one.
    direct: BTreeSet<PathBuf>,
    /// The directory entries that are symlinks to it, spelled the same way.
    linked: BTreeSet<PathBuf>,
    /// Whether it holds no model content (see
    /// [`DuplicateCandidate::bookkeeping`]).
    bookkeeping: bool,
}

/// The files of `candidate` by identity, or `None` when one is not a regular
/// file or its directory cannot be resolved.
fn files_of(candidate: &DuplicateCandidate) -> Option<BTreeMap<Identity, StoredFile>> {
    if candidate.files.is_empty() {
        return None;
    }
    let listed = candidate
        .files
        .iter()
        .map(|path| (path, false))
        .chain(candidate.bookkeeping.iter().map(|path| (path, true)));
    let mut files: BTreeMap<Identity, StoredFile> = BTreeMap::new();
    for (path, bookkeeping) in listed {
        let path = PathBuf::from(path);
        let metadata = std::fs::metadata(&path).ok().filter(Metadata::is_file)?;
        let entry = entry_of(&path)?;
        let file = files
            .entry(file_identity(&path, &metadata))
            .or_insert_with(|| StoredFile {
                path: path.clone(),
                size: metadata.len(),
                links: link_count(&metadata),
                direct: BTreeSet::new(),
                linked: BTreeSet::new(),
                bookkeeping,
            });
        file.bookkeeping &= bookkeeping;
        if is_symlink(&path) {
            file.linked.insert(entry);
        } else {
            file.direct.insert(entry);
        }
    }
    Some(files)
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// What makes two paths one file.
#[cfg(unix)]
pub(crate) type Identity = (u64, u64);

/// What makes two paths one file.
#[cfg(not(unix))]
pub(crate) type Identity = PathBuf;

/// The device and inode `metadata` (read following symlinks) names, so a hard
/// link is caught as well as a symlink.
#[cfg(unix)]
pub(crate) fn file_identity(_path: &Path, metadata: &Metadata) -> Identity {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

/// The canonical path, where there is no inode to read.
#[cfg(not(unix))]
pub(crate) fn file_identity(path: &Path, _metadata: &Metadata) -> Identity {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn link_count(metadata: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink()
}

/// One, where the link count cannot be read and a file is known by its
/// canonical path alone.
#[cfg(not(unix))]
fn link_count(_metadata: &Metadata) -> u64 {
    1
}

/// Split `files`, all of one size, into the classes of two or more whose
/// sampled content is the same. Every file is read one sample at a time, the
/// heads first, and a file stops being read as soon as no other still
/// matches it.
fn identical<'c>(files: &[(&'c Identity, &StoredFile)]) -> Vec<Vec<&'c Identity>> {
    let mut readings: Vec<Option<FileReading>> = files
        .iter()
        .map(|(_, file)| Some(FileReading::of(file)))
        .collect();
    let steps = readings
        .iter()
        .flatten()
        .map(|reading| reading.plan.len())
        .max()
        .unwrap_or(0);
    let mut classes: Vec<Vec<usize>> = vec![(0..files.len()).collect()];
    for step in 0..steps {
        let mut next: Vec<Vec<usize>> = Vec::new();
        for class in classes {
            let mut by_key: BTreeMap<(u64, [u8; 32]), Vec<usize>> = BTreeMap::new();
            for index in class {
                let key = readings[index]
                    .as_mut()
                    .and_then(|reading| reading.advance(step));
                match key {
                    Some(key) => by_key.entry(key).or_default().push(index),
                    None => readings[index] = None,
                }
            }
            next.extend(by_key.into_values().filter(|class| class.len() > 1));
        }
        classes = next;
        if classes.is_empty() {
            break;
        }
    }
    classes
        .into_iter()
        .map(|class| class.into_iter().map(|index| files[index].0).collect())
        .collect()
}

/// A file being read, sample by sample.
struct FileReading {
    path: PathBuf,
    size: u64,
    plan: Vec<(u64, u64)>,
    hasher: Sha256,
}

impl FileReading {
    fn of(file: &StoredFile) -> Self {
        Self {
            path: file.path.clone(),
            size: file.size,
            plan: samples(file.size),
            hasher: Sha256::new(),
        }
    }

    /// Read sample `step`, when the file has one, and return the size and
    /// the digest so far. `None` when the read fails.
    fn advance(&mut self, step: usize) -> Option<(u64, [u8; 32])> {
        if let Some(&(offset, length)) = self.plan.get(step) {
            hash_range(&self.path, offset, length, &mut self.hasher)?;
        }
        Some((self.size, self.hasher.clone().finalize().into()))
    }
}

/// The ranges [`detect`] reads of a file `size` bytes long, in order: the
/// head, then the blocks, or the whole file a block at a time when it is no
/// larger than the head and every block together.
fn samples(size: u64) -> Vec<(u64, u64)> {
    let mut plan = vec![(0, size.min(HEAD_BYTES))];
    if size <= HEAD_BYTES + BLOCKS * BLOCK_BYTES {
        let mut offset = HEAD_BYTES;
        while offset < size {
            plan.push((offset, BLOCK_BYTES.min(size - offset)));
            offset += BLOCK_BYTES;
        }
    } else {
        let span = u128::from(size - HEAD_BYTES - BLOCK_BYTES);
        for block in 0..BLOCKS {
            let step = span * u128::from(block) / u128::from(BLOCKS - 1);
            let step = u64::try_from(step).unwrap_or(0);
            plan.push((HEAD_BYTES + step, BLOCK_BYTES));
        }
    }
    plan
}

#[cfg(test)]
thread_local! {
    /// How many sampled ranges this thread has read.
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Feed `length` bytes of the file at `path` from `offset` into `hasher`.
fn hash_range(path: &Path, offset: u64, length: u64, hasher: &mut Sha256) -> Option<()> {
    #[cfg(test)]
    READS.with(|reads| reads.set(reads.get() + 1));
    let mut file = crate::fs::open_regular(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; usize::try_from(length).ok()?];
    file.read_exact(&mut buffer).ok()?;
    hasher.update(&buffer);
    Some(())
}

/// A content fingerprint for the file at `path`: the lowercase-hex SHA-256 of its
/// first and last megabyte (or the whole file, if small). `None` if unreadable.
///
/// Records store it, and a model that moved is matched to its old record by
/// it, so it stays this cheap rule: a stronger one would no longer match the
/// fingerprints records already hold. [`detect`] samples far more before it
/// calls two files copies.
pub fn content_fingerprint(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    fingerprint(path, size).map(hex::encode)
}

fn fingerprint(path: &Path, size: u64) -> Option<[u8; 32]> {
    let mut file = crate::fs::open_regular(path).ok()?;
    let mut hasher = Sha256::new();
    if size <= SAMPLE_SIZE * 2 {
        let mut whole = Vec::new();
        file.take(size).read_to_end(&mut whole).ok()?;
        hasher.update(&whole);
    } else {
        let mut head = vec![0u8; SAMPLE_SIZE as usize];
        file.read_exact(&mut head).ok()?;
        file.seek(SeekFrom::Start(size - SAMPLE_SIZE)).ok()?;
        let mut tail = vec![0u8; SAMPLE_SIZE as usize];
        file.read_exact(&mut tail).ok()?;
        hasher.update(&head);
        hasher.update(&tail);
    }
    Some(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: u64 = 300 << 20;

    /// A sparse file of [`SIZE`] bytes in a fresh directory, starting with
    /// `head`.
    fn sparse(name: &str, head: u8) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hedos-duplicates-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, [head]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(SIZE)
            .unwrap();
        path
    }

    fn candidate(path: &Path) -> DuplicateCandidate {
        let path = path.to_string_lossy().into_owned();
        DuplicateCandidate {
            member: DuplicateMember {
                id: path.clone(),
                name: path.clone(),
                kind: SourceKind::file(),
                path: path.clone(),
            },
            files: vec![path],
            bookkeeping: Vec::new(),
            pinned_by: Vec::new(),
        }
    }

    fn reads_of(candidates: &[DuplicateCandidate]) -> (usize, Vec<DuplicateGroup>) {
        READS.with(|reads| reads.set(0));
        let groups = detect(candidates, 1);
        (READS.with(std::cell::Cell::get), groups)
    }

    #[test]
    fn the_heads_are_read_first_and_a_difference_there_ends_the_reading() {
        let a = sparse("head-a.gguf", b'a');
        let b = sparse("head-b.gguf", b'b');
        let (reads, groups) = reads_of(&[candidate(&a), candidate(&b)]);
        std::fs::remove_dir_all(a.parent().unwrap()).ok();
        assert!(groups.is_empty());
        assert_eq!(reads, 2, "one head each, and nothing past it");
    }

    #[test]
    fn copies_that_agree_are_read_through_every_sample() {
        let a = sparse("same-a.gguf", b's');
        let b = sparse("same-b.gguf", b's');
        let (reads, groups) = reads_of(&[candidate(&a), candidate(&b)]);
        std::fs::remove_dir_all(a.parent().unwrap()).ok();
        assert_eq!(groups.len(), 1);
        assert_eq!(reads, 2 * (1 + BLOCKS as usize));
    }

    #[cfg(unix)]
    #[test]
    fn every_link_on_the_way_is_passed_a_directory_link_included() {
        let dir = std::env::temp_dir().join(format!(
            "hedos-passed-links-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real/sub")).unwrap();
        std::fs::write(dir.join("real/sub/w.gguf"), b"w").unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        std::os::unix::fs::symlink("real", dir.join("hop")).unwrap();
        std::os::unix::fs::symlink("../hop/sub/w.gguf", dir.join("real/link.gguf")).unwrap();
        std::os::unix::fs::symlink(dir.join("real/link.gguf"), dir.join("entry")).unwrap();
        let passed = passed_links(&dir.join("entry"));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            passed,
            vec![
                dir.join("entry"),
                dir.join("real/link.gguf"),
                dir.join("hop"),
                dir.join("real/sub/w.gguf"),
            ]
        );
    }

    #[test]
    fn samples_start_at_the_head_and_end_on_the_last_byte() {
        let plan = samples(SIZE);
        assert_eq!(plan[0], (0, HEAD_BYTES));
        assert_eq!(plan.len(), 1 + BLOCKS as usize);
        assert_eq!(plan[1], (HEAD_BYTES, BLOCK_BYTES));
        let (offset, length) = plan[plan.len() - 1];
        assert_eq!(offset + length, SIZE);
        assert_eq!(samples(100), vec![(0, 100)]);
        let small = samples(HEAD_BYTES + BLOCK_BYTES + 1);
        assert_eq!(
            small,
            vec![
                (0, HEAD_BYTES),
                (HEAD_BYTES, BLOCK_BYTES),
                (HEAD_BYTES + BLOCK_BYTES, 1)
            ]
        );
    }
}
