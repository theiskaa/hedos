//! Integration tests for the `discovery` pieces: GGUF shard-name parsing and
//! grouping, and content-fingerprint duplicate detection. Public API only.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use kernel::discovery::{
    DuplicateCandidate, DuplicateGroup, DuplicateMember, detect, group, parse, shard_filename,
};
use kernel::records::SourceKind;
use support::TempDir;

#[test]
fn parse_reads_a_well_formed_shard_filename() {
    let shard = parse("model-00002-of-00005.gguf").unwrap();
    assert_eq!(shard.base, "model");
    assert_eq!(shard.index, 2);
    assert_eq!(shard.total, 5);
}

#[test]
fn parse_is_case_insensitive_on_the_extension() {
    assert!(parse("model-00001-of-00003.GGUF").is_some());
}

#[test]
fn parse_rejects_malformed_names() {
    for bad in [
        "model.gguf",
        "model-1-of-5.gguf",
        "model-00001-of-005.gguf",
        "model-00006-of-00005.gguf",
        "model-00000-of-00005.gguf",
        "-00001-of-00005.gguf",
        "model-00001-of-00000.gguf",
        "model-00001-of-00005.bin",
        "model-0000a-of-00005.gguf",
    ] {
        assert!(parse(bad).is_none(), "should reject {bad}");
    }
}

#[test]
fn shard_filename_round_trips_through_parse() {
    let name = shard_filename("qwen", 3, 12);
    assert_eq!(name, "qwen-00003-of-00012.gguf");
    let shard = parse(&name).unwrap();
    assert_eq!(
        (shard.base.as_str(), shard.index, shard.total),
        ("qwen", 3, 12)
    );
}

fn shard_path(dir: &TempDir, base: &str, index: usize, total: usize) -> PathBuf {
    dir.join(&shard_filename(base, index, total))
}

#[test]
fn group_collects_members_and_loose_files() {
    let dir = TempDir::new();
    let files = vec![
        (shard_path(&dir, "m", 2, 3), 20),
        (shard_path(&dir, "m", 1, 3), 10),
        (shard_path(&dir, "m", 3, 3), 30),
        (dir.join("solo.gguf"), 5),
    ];
    let (groups, loose) = group(&files);
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert_eq!(g.base, "m");
    assert_eq!(g.total, 3);
    assert_eq!(
        g.members.iter().map(|m| m.index).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(g.complete());
    assert_eq!(g.footprint_bytes(), 60);
    assert_eq!(g.first_shard(), Some(shard_path(&dir, "m", 1, 3).as_path()));
    assert_eq!(loose, vec![dir.join("solo.gguf")]);
}

#[test]
fn group_marks_incomplete_sets_and_missing_first_shard() {
    let dir = TempDir::new();
    let files = vec![
        (shard_path(&dir, "m", 2, 3), 20),
        (shard_path(&dir, "m", 3, 3), 30),
    ];
    let (groups, _) = group(&files);
    assert_eq!(groups.len(), 1);
    assert!(!groups[0].complete());
    assert_eq!(groups[0].first_shard(), None);
}

#[test]
fn group_separates_different_totals_and_bases() {
    let dir = TempDir::new();
    let files = vec![
        (shard_path(&dir, "m", 1, 2), 1),
        (shard_path(&dir, "m", 1, 3), 1),
        (shard_path(&dir, "other", 1, 2), 1),
    ];
    let (groups, _) = group(&files);
    assert_eq!(groups.len(), 3);
}

fn make(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

fn member(name: &str, path: &Path) -> DuplicateMember {
    DuplicateMember {
        id: format!("id-{name}"),
        name: name.to_owned(),
        kind: SourceKind::file(),
        path: path.to_string_lossy().into_owned(),
    }
}

/// `member` as a candidate whose removal deletes its one file.
fn candidate(member: DuplicateMember) -> DuplicateCandidate {
    DuplicateCandidate {
        files: vec![member.path.clone()],
        member,
        bookkeeping: Vec::new(),
        pinned_by: Vec::new(),
    }
}

fn members(pairs: &[(&str, &PathBuf)]) -> Vec<DuplicateCandidate> {
    pairs
        .iter()
        .map(|(name, path)| candidate(member(name, path)))
        .collect()
}

/// A candidate named `name` whose removal deletes every one of `files`.
fn holding(name: &str, files: &[&PathBuf]) -> DuplicateCandidate {
    DuplicateCandidate {
        member: member(name, files[0]),
        files: files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        bookkeeping: Vec::new(),
        pinned_by: Vec::new(),
    }
}

/// The name of the copy `group` keeps, then of each copy it offers.
fn names(group: &DuplicateGroup) -> (&str, Vec<&str>) {
    (
        group.kept.member.name.as_str(),
        group
            .removable
            .iter()
            .map(|copy| copy.copy.member.name.as_str())
            .collect(),
    )
}

/// What removing each copy `group` offers frees, in order.
fn freed(group: &DuplicateGroup) -> Vec<i64> {
    group
        .removable
        .iter()
        .map(|copy| copy.reclaimable_bytes)
        .collect()
}

#[test]
fn detect_finds_identical_large_files() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 1000]);
    let b = make(&dir, "b.gguf", &[7u8; 1000]);
    let groups = detect(&members(&[("Bee", &b), ("Aay", &a)]), 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].reclaimable_bytes, 1000);
    assert_eq!(names(&groups[0]), ("Aay", vec!["Bee"]));
}

#[test]
fn detect_ignores_different_content_of_the_same_size() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[1u8; 1000]);
    let b = make(&dir, "b.gguf", &[2u8; 1000]);
    assert!(detect(&members(&[("A", &a), ("B", &b)]), 100).is_empty());
}

#[test]
fn detect_ignores_files_below_the_threshold() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 50]);
    let b = make(&dir, "b.gguf", &[7u8; 50]);
    assert!(detect(&members(&[("A", &a), ("B", &b)]), 100).is_empty());
}

#[test]
fn detect_reports_reclaimable_bytes_and_sorts_by_them() {
    let dir = TempDir::new();
    let small_a = make(&dir, "sa.gguf", &[1u8; 200]);
    let small_b = make(&dir, "sb.gguf", &[1u8; 200]);
    let big_a = make(&dir, "ba.gguf", &[9u8; 5000]);
    let big_b = make(&dir, "bb.gguf", &[9u8; 5000]);
    let big_c = make(&dir, "bc.gguf", &[9u8; 5000]);
    let groups = detect(
        &members(&[
            ("sa", &small_a),
            ("sb", &small_b),
            ("ba", &big_a),
            ("bb", &big_b),
            ("bc", &big_c),
        ]),
        100,
    );
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].reclaimable_bytes, 10_000);
    assert_eq!(groups[1].reclaimable_bytes, 200);
}

#[test]
fn detect_skips_missing_files() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 1000]);
    let ghost = dir.join("ghost.gguf");
    let groups = detect(&members(&[("A", &a), ("Ghost", &ghost)]), 100);
    assert!(groups.is_empty());
}

#[test]
fn parse_handles_tricky_but_valid_and_invalid_names() {
    let tricky = parse("a-of-b-00001-of-00005.gguf").unwrap();
    assert_eq!(
        (tricky.base.as_str(), tricky.index, tricky.total),
        ("a-of-b", 1, 5)
    );

    let equal = parse("model-00005-of-00005.gguf").unwrap();
    assert_eq!((equal.index, equal.total), (5, 5));

    assert!(parse("abc-of-00005.gguf").is_none());
    assert!(parse(".gguf").is_none());
    assert!(parse(".GGUF").is_none());
    assert!(parse("model-+0001-of-00005.gguf").is_none());
}

#[test]
fn group_dedups_duplicate_indices() {
    let dir = TempDir::new();
    let path = shard_path(&dir, "m", 1, 2);
    let files = vec![(path.clone(), 10), (path, 10)];
    let (groups, _) = group(&files);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 1, "a repeated index counts once");
    assert!(
        !groups[0].complete(),
        "a set missing shard 2 is not complete"
    );
    assert_eq!(
        groups[0].footprint_bytes(),
        10,
        "bytes are not double-counted"
    );
}

#[test]
fn detect_does_not_report_a_single_file_listed_twice() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 1000]);
    let groups = detect(&members(&[("First", &a), ("Second", &a)]), 100);
    assert!(
        groups.is_empty(),
        "the same path twice is not a real duplicate"
    );
}

#[test]
fn detect_reports_all_members_of_a_larger_group() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[3u8; 2000]);
    let b = make(&dir, "b.gguf", &[3u8; 2000]);
    let c = make(&dir, "c.gguf", &[3u8; 2000]);
    let groups = detect(&members(&[("A", &a), ("B", &b), ("C", &c)]), 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(names(&groups[0]), ("A", vec!["B", "C"]));
    assert_eq!(freed(&groups[0]), vec![2000, 2000]);
    assert_eq!(groups[0].reclaimable_bytes, 4000);
}

#[test]
fn detect_keeps_each_name_with_its_own_path() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[5u8; 1000]);
    let z = make(&dir, "z.gguf", &[5u8; 1000]);
    let groups = detect(&members(&[("Zed", &a), ("Amy", &z)]), 100);
    assert_eq!(groups.len(), 1);
    let amy = &groups[0].kept.member;
    assert_eq!(amy.name, "Amy");
    assert_eq!(amy.path, z.to_string_lossy());
    let zed = &groups[0].removable[0].copy.member;
    assert_eq!(zed.name, "Zed");
    assert_eq!(zed.path, a.to_string_lossy());
}

#[test]
fn detect_carries_each_members_id_and_store() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[6u8; 1000]);
    let b = make(&dir, "b.gguf", &[6u8; 1000]);
    let mut ollama = member("qwen", &a);
    ollama.id = "ollama-id".to_owned();
    ollama.kind = SourceKind::ollama();
    let mut studio = member("Qwen-Q4", &b);
    studio.id = "studio-id".to_owned();
    studio.kind = SourceKind::lm_studio();
    let groups = detect(&[candidate(studio.clone()), candidate(ollama.clone())], 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].kept.member, ollama,
        "Ollama is kept before LM Studio"
    );
    assert_eq!(groups[0].removable[0].copy.member, studio);
}

#[cfg(unix)]
#[test]
fn detect_counts_a_hard_link_as_one_copy() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[8u8; 1000]);
    let linked = dir.join("linked.gguf");
    fs::hard_link(&a, &linked).unwrap();
    assert!(detect(&members(&[("A", &a), ("Linked", &linked)]), 100).is_empty());
}

#[cfg(unix)]
#[test]
fn detect_counts_a_symlink_as_one_copy() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[8u8; 1000]);
    let linked = dir.join("linked.gguf");
    std::os::unix::fs::symlink(&a, &linked).unwrap();
    assert!(detect(&members(&[("A", &a), ("Linked", &linked)]), 100).is_empty());
}

#[cfg(unix)]
#[test]
fn detect_names_a_copy_by_its_file_when_a_symlink_comes_first() {
    let dir = TempDir::new();
    let target = make(&dir, "b.gguf", &[4u8; 1000]);
    let other = make(&dir, "a.gguf", &[4u8; 1000]);
    let link = dir.join("link.gguf");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let groups = detect(
        &members(&[("link", &link), ("other", &other), ("target", &target)]),
        100,
    );
    assert_eq!(groups.len(), 1);
    assert_eq!(names(&groups[0]), ("other", vec!["target"]));
    let copy = &groups[0].removable[0].copy;
    assert_eq!(copy.member.path, target.to_string_lossy());
    assert_eq!(copy.link_target, None);
    let aliases: Vec<&str> = copy
        .aliases
        .iter()
        .map(|alias| alias.name.as_str())
        .collect();
    assert_eq!(aliases, vec!["link"]);
    assert_eq!(groups[0].reclaimable_bytes, 1000);
}

#[cfg(unix)]
#[test]
fn detect_names_the_file_behind_a_copy_only_a_symlink_reaches() {
    let dir = TempDir::new();
    let hidden = make(&dir, "hidden.gguf", &[4u8; 1000]);
    let other = make(&dir, "a.gguf", &[4u8; 1000]);
    let link = dir.join("link.gguf");
    std::os::unix::fs::symlink(&hidden, &link).unwrap();
    let groups = detect(&members(&[("link", &link), ("other", &other)]), 100);
    assert_eq!(
        names(&groups[0]),
        ("link", vec!["other"]),
        "removing the link frees nothing, so it is the copy to keep"
    );
    let real = fs::canonicalize(&hidden).unwrap();
    assert_eq!(
        groups[0].kept.link_target.as_deref(),
        Some(&*real.to_string_lossy())
    );
    assert_eq!(groups[0].reclaimable_bytes, 1000);
}

#[cfg(unix)]
#[test]
fn detect_offers_nothing_when_every_copy_is_only_a_link() {
    let dir = TempDir::new();
    let one = make(&dir, "one.gguf", &[4u8; 1000]);
    let two = make(&dir, "two.gguf", &[4u8; 1000]);
    let first = dir.join("first.gguf");
    let second = dir.join("second.gguf");
    std::os::unix::fs::symlink(&one, &first).unwrap();
    std::os::unix::fs::symlink(&two, &second).unwrap();
    assert!(detect(&members(&[("first", &first), ("second", &second)]), 100).is_empty());
}

#[test]
fn detect_offers_a_loose_clone_of_one_quantization_and_never_the_repo() {
    let dir = TempDir::new();
    let q8 = make(&dir, "a-q8.gguf", &[8u8; 2000]);
    let q4 = make(&dir, "a-q4.gguf", &[4u8; 1000]);
    let q8_copy = make(&dir, "b-q8.gguf", &[8u8; 2000]);
    let mut repo = holding("repo", &[&q8, &q4]);
    repo.member.kind = SourceKind::huggingface_cache();
    let groups = detect(&[holding("loose", &[&q8_copy]), repo], 100);
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(
        names(&groups[0]),
        ("repo", vec!["loose"]),
        "the repo's Q4 has no copy in the loose file"
    );
    assert_eq!(freed(&groups[0]), vec![2000]);
}

#[test]
fn detect_offers_an_lm_studio_clone_of_an_ollama_model_and_never_the_ollama_model() {
    let dir = TempDir::new();
    let blob = make(&dir, "sha256-weights", &[2u8; 1000]);
    let template = make(&dir, "sha256-template", b"{{ .Prompt }}");
    let clone = make(&dir, "qwen-q4.gguf", &[2u8; 1000]);
    let mut ollama = holding("qwen:7b", &[&blob, &template]);
    ollama.member.kind = SourceKind::ollama();
    let mut studio = holding("Qwen-Q4", &[&clone]);
    studio.member.kind = SourceKind::lm_studio();
    let groups = detect(&[studio, ollama], 100);
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(
        names(&groups[0]),
        ("qwen:7b", vec!["Qwen-Q4"]),
        "the clone holds no template, so the Ollama model cannot go for it"
    );
    assert_eq!(groups[0].reclaimable_bytes, 1000);
}

#[test]
fn detect_keeps_one_of_two_identical_loose_files_by_store_then_name() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[6u8; 1000]);
    let b = make(&dir, "b.gguf", &[6u8; 1000]);
    let c = make(&dir, "c.gguf", &[6u8; 1000]);
    let mut repo = holding("zz-repo", &[&c]);
    repo.member.kind = SourceKind::huggingface_cache();
    let groups = detect(&members(&[("b", &b), ("a", &a)]), 100);
    assert_eq!(names(&groups[0]), ("a", vec!["b"]));
    let mut with_repo = members(&[("b", &b), ("a", &a)]);
    with_repo.push(repo);
    let groups = detect(&with_repo, 100);
    assert_eq!(groups.len(), 1, "one group, one kept: {groups:?}");
    assert_eq!(
        names(&groups[0]),
        ("zz-repo", vec!["a", "b"]),
        "the Hugging Face cache is kept before a loose file"
    );
    assert_eq!(groups[0].reclaimable_bytes, 2000);
}

#[cfg(unix)]
#[test]
fn detect_counts_only_the_files_a_partial_share_frees() {
    let dir = TempDir::new();
    let first = make(&dir, "p1-first.gguf", &[1u8; 1000]);
    let second = make(&dir, "p1-second.gguf", &[2u8; 1000]);
    let shared = dir.join("p2-first.gguf");
    fs::hard_link(&first, &shared).unwrap();
    let cloned = make(&dir, "p2-second.gguf", &[2u8; 1000]);
    let groups = detect(
        &[
            holding("p1", &[&first, &second]),
            holding("p2", &[&shared, &cloned]),
        ],
        100,
    );
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(names(&groups[0]), ("p1", vec!["p2"]));
    assert_eq!(
        freed(&groups[0]),
        vec![1000],
        "the shared first file stays with p1, so only the second is freed"
    );
}

#[cfg(unix)]
#[test]
fn detect_never_offers_a_file_a_symlink_on_the_shelf_reaches() {
    let dir = TempDir::new();
    let file = make(&dir, "file.gguf", &[5u8; 1000]);
    let clone = make(&dir, "clone.gguf", &[5u8; 1000]);
    let link = dir.join("link.gguf");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let groups = detect(
        &members(&[("a-link", &link), ("b-file", &file), ("c-clone", &clone)]),
        100,
    );
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(
        names(&groups[0]),
        ("b-file", vec!["c-clone"]),
        "removing b-file would leave a-link dangling"
    );
}

#[cfg(unix)]
#[test]
fn detect_offers_a_partial_share_and_a_partial_link_for_the_set_they_lean_on() {
    let dir = TempDir::new();
    let first = make(&dir, "p1-first.gguf", &[1u8; 1000]);
    let second = make(&dir, "p1-second.gguf", &[2u8; 1000]);
    let shared = dir.join("p2-first.gguf");
    fs::hard_link(&first, &shared).unwrap();
    let cloned = make(&dir, "p2-second.gguf", &[2u8; 1000]);
    let linked = dir.join("p3-first.gguf");
    std::os::unix::fs::symlink(&first, &linked).unwrap();
    let again = make(&dir, "p3-second.gguf", &[2u8; 1000]);
    let groups = detect(
        &[
            holding("p1", &[&first, &second]),
            holding("p2", &[&shared, &cloned]),
            holding("p3", &[&linked, &again]),
        ],
        100,
    );
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(
        names(&groups[0]),
        ("p1", vec!["p2", "p3"]),
        "p1 cannot go while p3 links to its first file"
    );
    assert_eq!(freed(&groups[0]), vec![1000, 1000]);
}

#[cfg(unix)]
#[test]
fn detect_keeps_a_copy_hard_linked_where_no_candidate_is() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[8u8; 1000]);
    let b = make(&dir, "b.gguf", &[8u8; 1000]);
    fs::hard_link(&a, dir.join("elsewhere.gguf")).unwrap();
    let groups = detect(&members(&[("b", &b), ("a", &a)]), 100);
    assert_eq!(
        names(&groups[0]),
        ("a", vec!["b"]),
        "removing a frees nothing, so a is kept"
    );
    assert_eq!(groups[0].reclaimable_bytes, 1000);
}

#[test]
fn detect_compares_every_file_of_a_set_however_many_differ() {
    let dir = TempDir::new();
    let set = |sub: &str, second: u8| {
        let folder = dir.join(sub);
        fs::create_dir_all(&folder).unwrap();
        let config = folder.join("config.json");
        let first = folder.join("model-00001-of-00002.safetensors");
        let other = folder.join("model-00002-of-00002.safetensors");
        fs::write(&config, b"{}").unwrap();
        fs::write(&first, [1u8; 600]).unwrap();
        fs::write(&other, vec![second; 600]).unwrap();
        holding(sub, &[&first, &other, &config])
    };
    let groups = detect(&[set("fb1", 2), set("fb2", 3), set("fb3", 2)], 100);
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(names(&groups[0]), ("fb1", vec!["fb3"]));
    assert_eq!(groups[0].reclaimable_bytes, 1202);
}

#[cfg(unix)]
#[test]
fn detect_keeps_a_hard_link_beside_the_copy_it_shares() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[8u8; 1000]);
    let b = make(&dir, "b.gguf", &[8u8; 1000]);
    let c = dir.join("c.gguf");
    fs::hard_link(&a, &c).unwrap();
    let groups = detect(&members(&[("a", &a), ("c", &c), ("b", &b)]), 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(names(&groups[0]), ("a", vec!["b"]), "a and c are one file");
    assert_eq!(groups[0].reclaimable_bytes, 1000);
    let shared = &groups[0].kept;
    assert_eq!(shared.member.name, "a");
    assert_eq!(shared.aliases.len(), 1);
    assert_eq!(shared.aliases[0].path, c.to_string_lossy());
}

#[test]
fn detect_keeps_two_ollama_tags_over_one_blob_as_one_copy() {
    let dir = TempDir::new();
    let blob = make(&dir, "sha256-abc", &[2u8; 1000]);
    let studio = make(&dir, "q.gguf", &[2u8; 1000]);
    let tag = |name: &str| {
        let mut tag = member(name, &blob);
        tag.kind = SourceKind::ollama();
        candidate(tag)
    };
    let mut copy = member("Qwen-Q4", &studio);
    copy.kind = SourceKind::lm_studio();
    let groups = detect(&[tag("qwen:latest"), candidate(copy), tag("qwen:7b")], 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].reclaimable_bytes, 1000);
    assert_eq!(names(&groups[0]), ("qwen:7b", vec!["Qwen-Q4"]));
    let aliases: Vec<&str> = groups[0]
        .kept
        .aliases
        .iter()
        .map(|alias| alias.name.as_str())
        .collect();
    assert_eq!(aliases, vec!["qwen:latest"]);
}

/// A sparse file of `size` bytes holding `head` over its first 8 MiB, `fill`
/// over the 16 MiB starting at its middle, and `tail` over its last MiB.
fn sampled_file(dir: &TempDir, name: &str, size: u64, fill: u8) -> PathBuf {
    use std::io::{Seek, SeekFrom, Write};
    const MIB: u64 = 1 << 20;
    let path = dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    file.set_len(size).unwrap();
    file.write_all(&vec![b'H'; (8 * MIB) as usize]).unwrap();
    file.seek(SeekFrom::Start(size / 2)).unwrap();
    file.write_all(&vec![fill; (16 * MIB) as usize]).unwrap();
    file.seek(SeekFrom::Start(size - MIB)).unwrap();
    file.write_all(&vec![b'T'; MIB as usize]).unwrap();
    path
}

#[test]
fn detect_tells_apart_files_that_differ_only_in_the_middle() {
    let dir = TempDir::new();
    let size = 256 << 20;
    let a = sampled_file(&dir, "a.gguf", size, b'x');
    let b = sampled_file(&dir, "b.gguf", size, b'y');
    let c = sampled_file(&dir, "c.gguf", size, b'x');
    let groups = detect(&members(&[("a", &a), ("b", &b), ("c", &c)]), 100);
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(names(&groups[0]), ("a", vec!["c"]));
}

/// A two-shard set in `dir/sub`, its second shard filled with `second`.
fn shard_set(dir: &TempDir, sub: &str, second: u8) -> DuplicateCandidate {
    let folder = dir.join(sub);
    fs::create_dir_all(&folder).unwrap();
    let first = folder.join(shard_filename("m", 1, 2));
    let other = folder.join(shard_filename("m", 2, 2));
    fs::write(&first, [1u8; 600]).unwrap();
    fs::write(&other, vec![second; 400]).unwrap();
    holding(sub, &[&first, &other])
}

#[test]
fn detect_compares_every_shard_of_a_set_and_counts_them_all() {
    let dir = TempDir::new();
    let groups = detect(
        &[
            shard_set(&dir, "one", 2),
            shard_set(&dir, "two", 3),
            shard_set(&dir, "three", 2),
        ],
        100,
    );
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(names(&groups[0]), ("one", vec!["three"]));
    assert_eq!(groups[0].reclaimable_bytes, 1000);
}

#[test]
fn detect_breaks_a_tie_between_groups_by_name() {
    let dir = TempDir::new();
    let b1 = make(&dir, "b1.gguf", &[1u8; 1000]);
    let b2 = make(&dir, "b2.gguf", &[1u8; 1000]);
    let a1 = make(&dir, "a1.gguf", &[9u8; 1000]);
    let a2 = make(&dir, "a2.gguf", &[9u8; 1000]);
    let groups = detect(
        &members(&[
            ("tie-b1", &b1),
            ("tie-b2", &b2),
            ("tie-a1", &a1),
            ("tie-a2", &a2),
        ]),
        100,
    );
    let firsts: Vec<&str> = groups.iter().map(|group| names(group).0).collect();
    assert_eq!(firsts, vec!["tie-a1", "tie-b1"]);
}
