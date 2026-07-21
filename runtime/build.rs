//! Packs `runtime/runtimes/` into one gzip-compressed archive at build time, so
//! `bundled.rs` can embed a single small blob instead of the raw tree, which is
//! dominated by `requirements.lock` files.
//!
//! Archive format: for each file, in deterministic sorted-by-path order,
//! `u32 LE path_byte_len`, the relative path (forward-slash separated, relative
//! to `runtimes/`), `u32 LE content_len`, the content bytes. The concatenation is
//! gzip-compressed as a whole.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;

fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let runtimes_root = Path::new(&manifest_dir).join("runtimes");
    println!("cargo:rerun-if-changed=runtimes");

    let mut files = BTreeMap::new();
    collect_files(&runtimes_root, &runtimes_root, &mut files);

    let mut payload = Vec::new();
    for (relative_path, contents) in &files {
        let path_bytes = relative_path.as_bytes();
        payload.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(path_bytes);
        payload.extend_from_slice(&(contents.len() as u32).to_le_bytes());
        payload.extend_from_slice(contents);
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&payload)
        .expect("writing to an in-memory gzip encoder cannot fail");
    let compressed = encoder
        .finish()
        .expect("finishing an in-memory gzip stream cannot fail");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let archive_path = Path::new(&out_dir).join("runtimes.archive.gz");
    std::fs::write(&archive_path, compressed).expect("writing the archive to OUT_DIR cannot fail");
}

/// Recursively collect every file under `dir`, keyed by its path relative to
/// `root` (forward-slash separated), into `out`. A `BTreeMap` keeps insertion
/// order irrelevant: iteration is always sorted by key, so the archive is
/// byte-identical across builds regardless of the OS directory-read order.
fn collect_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|err| panic!("reading entry in {}: {err}", dir.display()))
                .path()
        })
        .collect();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            collect_files(root, &path, out);
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or_else(|err| {
            panic!(
                "stripping {} from {}: {err}",
                root.display(),
                path.display()
            )
        });
        let relative = relative
            .to_str()
            .unwrap_or_else(|| panic!("non-utf8 path: {}", relative.display()))
            .replace(std::path::MAIN_SEPARATOR, "/");
        let contents =
            std::fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        out.insert(relative, contents);
    }
}
