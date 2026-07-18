//! Recognizing and grouping multi-part GGUF weight files named like
//! `model-00001-of-00005.gguf`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The parts of a sharded GGUF filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardName {
    /// The base name shared by every shard in the set.
    pub base: String,
    /// This shard's 1-based index.
    pub index: usize,
    /// The total number of shards declared in the name.
    pub total: usize,
}

/// One member file of a shard group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The shard's 1-based index.
    pub index: usize,
    /// The file's path.
    pub path: PathBuf,
    /// The file's size in bytes.
    pub bytes: i64,
}

/// A set of shards that together form one model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardGroup {
    /// The shared base name.
    pub base: String,
    /// The declared total shard count.
    pub total: usize,
    /// The member files present, sorted by index.
    pub members: Vec<Member>,
}

impl ShardGroup {
    /// The path of the first shard (index 1), if present.
    pub fn first_shard(&self) -> Option<&Path> {
        self.members
            .iter()
            .find(|member| member.index == 1)
            .map(|member| member.path.as_path())
    }

    /// The total size of all present members.
    pub fn footprint_bytes(&self) -> i64 {
        self.members.iter().map(|member| member.bytes).sum()
    }

    /// Whether every declared shard is present.
    pub fn complete(&self) -> bool {
        self.members.len() == self.total
    }
}

/// Parse a `<base>-<index>-of-<total>.gguf` filename. The index and total fields
/// are exactly five digits, `1 <= index <= total`, and the base is non-empty.
pub fn parse(filename: &str) -> Option<ShardName> {
    let cut = filename.len().checked_sub(".gguf".len())?;
    if !filename.get(cut..)?.eq_ignore_ascii_case(".gguf") {
        return None;
    }
    let stem = &filename[..cut];
    let of_position = stem.rfind("-of-")?;
    let total_field = &stem[of_position + "-of-".len()..];
    let total = five_digit_number(total_field)?;
    if total == 0 {
        return None;
    }
    let head = &stem[..of_position];
    let dash_position = head.rfind('-')?;
    let index_field = &head[dash_position + 1..];
    let index = five_digit_number(index_field)?;
    if index == 0 || index > total {
        return None;
    }
    let base = &head[..dash_position];
    if base.is_empty() {
        return None;
    }
    Some(ShardName {
        base: base.to_owned(),
        index,
        total,
    })
}

/// Build the canonical filename for a shard.
pub fn shard_filename(base: &str, index: usize, total: usize) -> String {
    format!("{base}-{index:05}-of-{total:05}.gguf")
}

/// Split files into shard groups and loose (non-sharded) files. Shards are
/// grouped by directory, base name, and declared total, and each group's members
/// are sorted by index.
pub fn group(files: &[(PathBuf, i64)]) -> (Vec<ShardGroup>, Vec<PathBuf>) {
    let mut buckets: BTreeMap<(String, String, usize), Vec<Member>> = BTreeMap::new();
    let mut loose: Vec<PathBuf> = Vec::new();

    for (path, bytes) in files {
        let shard = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(parse);
        match shard {
            Some(shard) => {
                let directory = path
                    .parent()
                    .map(|dir| dir.to_string_lossy().into_owned())
                    .unwrap_or_default();
                buckets
                    .entry((directory, shard.base, shard.total))
                    .or_default()
                    .push(Member {
                        index: shard.index,
                        path: path.clone(),
                        bytes: *bytes,
                    });
            }
            None => loose.push(path.clone()),
        }
    }

    let groups = buckets
        .into_iter()
        .map(|((_, base, total), members)| {
            let mut seen = BTreeSet::new();
            let mut members: Vec<Member> = members
                .into_iter()
                .filter(|member| seen.insert(member.index))
                .collect();
            members.sort_by_key(|member| member.index);
            ShardGroup {
                base,
                total,
                members,
            }
        })
        .collect();
    (groups, loose)
}

fn five_digit_number(field: &str) -> Option<usize> {
    if field.len() != 5 || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}
