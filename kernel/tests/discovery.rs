//! Integration tests for the `discovery` pieces: GGUF shard-name parsing and
//! grouping, and content-fingerprint duplicate detection. Public API only.

mod support;

use std::fs;
use std::path::PathBuf;

use kernel::discovery::{detect, group, parse, shard_filename};
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

#[test]
fn detect_finds_identical_large_files() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 1000]);
    let b = make(&dir, "b.gguf", &[7u8; 1000]);
    let groups = detect(&[("Bee".into(), b), ("Aay".into(), a)], 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].wasted_bytes, 1000);
    assert_eq!(groups[0].names, vec!["Aay", "Bee"]);
    assert_eq!(groups[0].paths.len(), 2);
}

#[test]
fn detect_ignores_different_content_of_the_same_size() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[1u8; 1000]);
    let b = make(&dir, "b.gguf", &[2u8; 1000]);
    assert!(detect(&[("A".into(), a), ("B".into(), b)], 100).is_empty());
}

#[test]
fn detect_ignores_files_below_the_threshold() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 50]);
    let b = make(&dir, "b.gguf", &[7u8; 50]);
    assert!(detect(&[("A".into(), a), ("B".into(), b)], 100).is_empty());
}

#[test]
fn detect_reports_wasted_bytes_and_sorts_by_size() {
    let dir = TempDir::new();
    let small_a = make(&dir, "sa.gguf", &[1u8; 200]);
    let small_b = make(&dir, "sb.gguf", &[1u8; 200]);
    let big_a = make(&dir, "ba.gguf", &[9u8; 5000]);
    let big_b = make(&dir, "bb.gguf", &[9u8; 5000]);
    let big_c = make(&dir, "bc.gguf", &[9u8; 5000]);
    let groups = detect(
        &[
            ("sa".into(), small_a),
            ("sb".into(), small_b),
            ("ba".into(), big_a),
            ("bb".into(), big_b),
            ("bc".into(), big_c),
        ],
        100,
    );
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].wasted_bytes, 10_000);
    assert_eq!(groups[1].wasted_bytes, 200);
}

#[test]
fn detect_skips_missing_files() {
    let dir = TempDir::new();
    let a = make(&dir, "a.gguf", &[7u8; 1000]);
    let groups = detect(
        &[("A".into(), a), ("Ghost".into(), dir.join("ghost.gguf"))],
        100,
    );
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
    let groups = detect(&[("First".into(), a.clone()), ("Second".into(), a)], 100);
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
    let groups = detect(&[("A".into(), a), ("B".into(), b), ("C".into(), c)], 100);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].names.len(), 3);
    assert_eq!(groups[0].paths.len(), 3);
    assert_eq!(groups[0].wasted_bytes, 4000);
}
