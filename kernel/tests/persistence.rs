//! Integration tests for the `persistence` store substrate: atomic writes,
//! absent/corrupt/empty read semantics, quarantine behavior, temp-file cleanup
//! on failure, and concurrent writers. Exercises the public API only.

mod support;

use std::fs;

use kernel::persistence::{StoreError, quarantine, read_json, write_atomic, write_json_atomic};
use serde::{Deserialize, Serialize};
use support::TempDir;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    a: u32,
    b: String,
}

fn sample() -> Sample {
    Sample {
        a: 7,
        b: "hello".into(),
    }
}

#[test]
fn write_then_read_roundtrips() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    write_json_atomic(&path, &sample()).unwrap();
    let read: Option<Sample> = read_json(&path).unwrap();
    assert_eq!(read, Some(sample()));
}

#[test]
fn reading_absent_file_is_none() {
    let dir = TempDir::new();
    let read: Option<Sample> = read_json(&dir.join("missing.json")).unwrap();
    assert_eq!(read, None);
}

#[test]
fn write_creates_missing_parent_directories() {
    let dir = TempDir::new();
    let path = dir.join("nested/deeper/store.json");
    write_json_atomic(&path, &sample()).unwrap();
    assert!(path.exists());
}

#[test]
fn atomic_write_overwrites_existing() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    write_json_atomic(
        &path,
        &Sample {
            a: 1,
            b: "one".into(),
        },
    )
    .unwrap();
    write_json_atomic(
        &path,
        &Sample {
            a: 2,
            b: "two".into(),
        },
    )
    .unwrap();
    let read: Option<Sample> = read_json(&path).unwrap();
    assert_eq!(
        read,
        Some(Sample {
            a: 2,
            b: "two".into()
        })
    );
}

#[test]
fn corrupt_file_is_quarantined_and_reported() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    let garbage = b"{ this is not json";
    fs::write(&path, garbage).unwrap();

    match read_json::<Sample>(&path) {
        Err(StoreError::Corrupt {
            path: reported,
            quarantined: Some(moved),
            ..
        }) => {
            assert_eq!(reported, path);
            assert!(!path.exists(), "corrupt original should be moved aside");
            assert!(moved.exists(), "quarantined copy should exist");
            assert_eq!(fs::read(&moved).unwrap(), garbage);
            let name = moved.file_name().unwrap().to_str().unwrap();
            assert!(
                name.starts_with("store.json.corrupt-"),
                "unexpected name {name}"
            );
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn empty_file_is_corrupt_not_absent() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    fs::write(&path, b"").unwrap();
    assert!(matches!(
        read_json::<Sample>(&path),
        Err(StoreError::Corrupt { .. })
    ));
}

#[test]
fn encode_failure_is_reported_without_touching_disk() {
    use std::collections::HashMap;
    let dir = TempDir::new();
    let path = dir.join("store.json");
    let mut unencodable: HashMap<Vec<u8>, u32> = HashMap::new();
    unencodable.insert(vec![1, 2, 3], 9);

    let result = write_json_atomic(&path, &unencodable);
    assert!(matches!(result, Err(StoreError::Encode(_))));
    assert!(
        !path.exists(),
        "nothing should be written on encode failure"
    );
}

#[test]
fn successful_writes_leave_no_temp_files_behind() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    for n in 0..25 {
        write_json_atomic(
            &path,
            &Sample {
                a: n,
                b: n.to_string(),
            },
        )
        .unwrap();
    }
    assert_eq!(
        entries_besides(dir.path(), "store.json"),
        Vec::<String>::new()
    );
}

#[test]
fn failed_write_leaves_no_temp_file_behind() {
    let dir = TempDir::new();
    let dest = dir.join("dest");
    fs::create_dir(&dest).unwrap();

    let result = write_atomic(&dest, b"payload");
    assert!(
        result.is_err(),
        "renaming a file over a directory should fail"
    );

    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "temp file leaked: {leftovers:?}");
}

#[test]
fn reading_a_directory_path_is_io_error_not_absent() {
    let dir = TempDir::new();
    let path = dir.join("adir");
    fs::create_dir(&path).unwrap();
    assert!(matches!(read_json::<Sample>(&path), Err(StoreError::Io(_))));
    assert!(path.is_dir(), "the directory must not be quarantined");
}

#[test]
fn parent_component_that_is_a_file_surfaces_io_error() {
    let dir = TempDir::new();
    let file = dir.join("blocker");
    fs::write(&file, b"x").unwrap();
    let path = file.join("child.json");
    assert!(matches!(
        write_json_atomic(&path, &sample()),
        Err(StoreError::Io(_))
    ));
}

#[test]
fn quarantine_moves_file_and_returns_new_path() {
    let dir = TempDir::new();
    let path = dir.join("thing.json");
    fs::write(&path, b"payload").unwrap();
    let moved = quarantine(&path).unwrap();
    assert!(!path.exists());
    assert!(moved.exists());
    assert_eq!(fs::read(&moved).unwrap(), b"payload");
}

#[test]
fn quarantine_names_do_not_collide() {
    let dir = TempDir::new();
    let mut names = std::collections::HashSet::new();
    for n in 0..50 {
        let path = dir.join("dup.json");
        fs::write(&path, n.to_string()).unwrap();
        let moved = quarantine(&path).unwrap();
        let name = moved.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(names.insert(name), "duplicate quarantine name generated");
    }
    assert_eq!(names.len(), 50);
}

#[test]
fn concurrent_writers_yield_one_valid_value() {
    let dir = TempDir::new();
    let path = dir.join("store.json");
    std::thread::scope(|scope| {
        for n in 0..16u32 {
            let path = path.clone();
            scope.spawn(move || {
                write_json_atomic(
                    &path,
                    &Sample {
                        a: n,
                        b: n.to_string(),
                    },
                )
                .unwrap();
            });
        }
    });
    let read: Sample = read_json(&path).unwrap().expect("store must exist");
    assert!(read.a < 16, "unexpected value {}", read.a);
    assert_eq!(read.b, read.a.to_string());
    assert_eq!(
        entries_besides(dir.path(), "store.json"),
        Vec::<String>::new()
    );
}

#[cfg(unix)]
#[test]
fn quarantine_moves_symlink_not_target() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new();
    let target = dir.join("real.json");
    fs::write(&target, b"real bytes").unwrap();
    let link = dir.join("link.json");
    symlink(&target, &link).unwrap();

    let moved = quarantine(&link).unwrap();
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "the link should be gone from its path"
    );
    assert!(target.exists(), "the symlink target must be untouched");
    assert_eq!(fs::read(&target).unwrap(), b"real bytes");
    assert!(
        fs::symlink_metadata(&moved)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

fn entries_besides(dir: &std::path::Path, keep: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name != keep)
        .collect()
}
