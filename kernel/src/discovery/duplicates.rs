//! Detecting duplicate model weights by size then a cheap content fingerprint.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The default minimum file size (256 MiB) considered for duplicate detection —
/// small files are not worth reporting.
pub const DEFAULT_THRESHOLD: i64 = 256 << 20;

const SAMPLE_SIZE: u64 = 1 << 20;

/// A set of files found to be duplicates of one another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// The display names of the duplicate models, sorted.
    pub names: Vec<String>,
    /// The paths of the duplicate files, sorted.
    pub paths: Vec<String>,
    /// The bytes that could be reclaimed by keeping just one copy.
    pub wasted_bytes: i64,
}

/// Find groups of duplicate weight files among `candidates` (name, path pairs).
/// Files smaller than `threshold` are ignored. Candidates are bucketed by exact
/// size, then confirmed by a fingerprint over the first and last megabyte, so
/// only same-size, same-fingerprint files are reported. Groups are returned
/// most-wasteful first.
pub fn detect(candidates: &[(String, PathBuf)], threshold: i64) -> Vec<DuplicateGroup> {
    let mut seen_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut by_size: BTreeMap<i64, Vec<(&str, &Path)>> = BTreeMap::new();
    for (name, path) in candidates {
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !seen_paths.insert(identity) {
            continue;
        }
        let Ok(size) = file_size(path) else {
            continue;
        };
        if size >= threshold && size > 0 {
            by_size.entry(size).or_default().push((name, path));
        }
    }

    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for (size, bucket) in by_size {
        if bucket.len() < 2 {
            continue;
        }
        let mut by_fingerprint: BTreeMap<[u8; 32], Vec<(&str, &Path)>> = BTreeMap::new();
        for (name, path) in bucket {
            if let Some(digest) = fingerprint(path, size as u64) {
                by_fingerprint.entry(digest).or_default().push((name, path));
            }
        }
        for matches in by_fingerprint.into_values() {
            if matches.len() < 2 {
                continue;
            }
            let mut names: Vec<String> =
                matches.iter().map(|(name, _)| (*name).to_owned()).collect();
            let mut paths: Vec<String> = matches
                .iter()
                .map(|(_, path)| path.to_string_lossy().into_owned())
                .collect();
            names.sort();
            paths.sort();
            groups.push(DuplicateGroup {
                names,
                paths,
                wasted_bytes: (matches.len() as i64 - 1) * size,
            });
        }
    }

    groups.sort_by_key(|group| std::cmp::Reverse(group.wasted_bytes));
    groups
}

fn file_size(path: &Path) -> std::io::Result<i64> {
    Ok(std::fs::metadata(path)?.len() as i64)
}

/// A content fingerprint for the file at `path`: the lowercase-hex SHA-256 of its
/// first and last megabyte (or the whole file, if small). `None` if unreadable.
/// Two files with the same fingerprint and size are treated as the same weights.
pub fn content_fingerprint(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    fingerprint(path, size).map(hex::encode)
}

fn fingerprint(path: &Path, size: u64) -> Option<[u8; 32]> {
    let mut file = File::open(path).ok()?;
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
