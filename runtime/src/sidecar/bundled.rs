//! The Python runtime bundles shipped inside the binary. The Swift build carried
//! these as app resources; a headless binary has no resource bundle, so the
//! `runtime/runtimes` tree (each `python-*` bundle's `main.py`, `manifest.toml`,
//! and `requirements.*`) is packed by `build.rs` into one gzip-compressed archive,
//! embedded here, and extracted into the data directory on first use, where
//! [`RuntimeBundle`](super::RuntimeBundle) then locates it.

use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::Read;

/// The gzip-compressed archive of `runtime/runtimes/`, built by `build.rs` (see
/// that file for the archive format: length-prefixed path/content pairs in
/// deterministic sorted-path order, then gzipped as a whole).
const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/runtimes.archive.gz"));

/// Name of the marker file, written as a sibling of `Runtimes` inside the bundles
/// root, that records the hash of the tree last provisioned there.
const STAMP_FILE: &str = ".bundle-stamp";

/// Extract every embedded bundle under `bundles_root/Runtimes`, writing only files
/// that are missing or whose contents differ, so a shipped update refreshes the
/// bundle while leaving an up-to-date tree (and its mtimes) untouched.
///
/// A `.bundle-stamp` file next to `Runtimes` records the hash of the tree this
/// binary last provisioned there; when it already matches, the whole per-file
/// compare is skipped. Every embedded file is small but `requirements.lock` files
/// dominate the tree, so this turns the common "nothing changed since last boot"
/// case from reading and comparing ~3/4 MB into reading one short marker.
///
/// Best-effort and idempotent: the caller runs it on every boot. A failure leaves
/// the bundle absent, which surfaces later as the usual "runtime bundle is missing".
pub fn provision(bundles_root: &Path) -> std::io::Result<()> {
    let stamp_path = bundles_root.join(STAMP_FILE);
    let current = bundle_stamp();
    if std::fs::read_to_string(&stamp_path).is_ok_and(|stamp| stamp == current) {
        return Ok(());
    }

    write_entries(&decode_archive(), &bundles_root.join("Runtimes"))?;

    // Best-effort: a failed stamp write just means the next boot re-runs the
    // (correct, if slower) per-file compare instead of trusting a stale marker.
    let _ = std::fs::write(&stamp_path, &current);
    Ok(())
}

/// A hash over the embedded archive's compressed bytes, so it changes whenever
/// any embedded file does (the archive is rebuilt whenever `runtime/runtimes`
/// changes). Deterministic per build: `build.rs` packs the tree in sorted-path
/// order, so the same source tree always produces the same archive bytes.
fn bundle_stamp() -> String {
    hex::encode(Sha256::digest(ARCHIVE))
}

/// Gzip-inflate [`ARCHIVE`] and parse it back into `(relative path, contents)`
/// pairs. `build.rs` documents the exact length-prefixed layout; this is its
/// mirror image. Any malformed archive (impossible for our own build output, but
/// this must never panic) yields an empty `Vec` rather than propagating a parse
/// error the caller has no way to act on.
fn decode_archive() -> Vec<(PathBuf, Vec<u8>)> {
    let mut inflated = Vec::new();
    if GzDecoder::new(ARCHIVE).read_to_end(&mut inflated).is_err() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut cursor = &inflated[..];
    while !cursor.is_empty() {
        let Some(path) = read_len_prefixed(&mut cursor) else {
            return Vec::new();
        };
        let Ok(path) = String::from_utf8(path) else {
            return Vec::new();
        };
        let Some(contents) = read_len_prefixed(&mut cursor) else {
            return Vec::new();
        };
        entries.push((PathBuf::from(path), contents));
    }
    entries
}

/// Read a `u32 LE` length prefix followed by that many bytes from the front of
/// `cursor`, advancing it past what was read. `None` on a truncated/malformed
/// buffer.
fn read_len_prefixed(cursor: &mut &[u8]) -> Option<Vec<u8>> {
    if cursor.len() < 4 {
        return None;
    }
    let (len_bytes, rest) = cursor.split_at(4);
    let len = u32::from_le_bytes(len_bytes.try_into().ok()?) as usize;
    if rest.len() < len {
        return None;
    }
    let (value, rest) = rest.split_at(len);
    *cursor = rest;
    Some(value.to_vec())
}

/// Write every `(relative path, contents)` pair into `dest`, keyed by the path
/// relative to the embedded root (so `python-mlx-audio/main.py` lands at
/// `dest/python-mlx-audio/main.py`).
fn write_entries(entries: &[(PathBuf, Vec<u8>)], dest: &Path) -> std::io::Result<()> {
    for (path, contents) in entries {
        let target = dest.join(path);
        if is_current(&target, contents) {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Write then rename so a concurrently-booting process never reads a
        // half-written bundle file. The temp name is keyed to the full file
        // name so sibling files (requirements.in vs .lock) never collide.
        let temp = match target.file_name().and_then(|name| name.to_str()) {
            Some(name) => target.with_file_name(format!(".{name}.part")),
            None => continue,
        };
        std::fs::write(&temp, contents)?;
        std::fs::rename(&temp, &target)?;
    }
    Ok(())
}

/// Whether `target` already holds exactly `contents`.
fn is_current(target: &Path, contents: &[u8]) -> bool {
    std::fs::read(target).is_ok_and(|existing| existing == contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hedos-bundled-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn provision_writes_the_known_bundles() {
        let root = temp_root("write");
        provision(&root).unwrap();
        for name in [
            "python-mlx-audio",
            "python-diffusers",
            "python-mlx-lm",
            "python-mlx-vlm",
            "python-embeddings",
            "python-mflux",
            "python-whisper-cpp",
        ] {
            let main = root.join("Runtimes").join(name).join("main.py");
            assert!(main.exists(), "{name} main.py should be extracted");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn provision_is_idempotent_and_refreshes_changed_files() {
        let root = temp_root("refresh");
        provision(&root).unwrap();

        let main = root
            .join("Runtimes")
            .join("python-mlx-audio")
            .join("main.py");
        let shipped = std::fs::read(&main).unwrap();

        // A local edit is overwritten back to the shipped contents on the next run,
        // once the stamp no longer short-circuits the per-file compare (the stamp
        // fast path itself is covered separately below).
        std::fs::write(&main, b"tampered").unwrap();
        std::fs::remove_file(root.join(STAMP_FILE)).unwrap();
        provision(&root).unwrap();
        assert_eq!(std::fs::read(&main).unwrap(), shipped);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn provision_writes_a_stamp_on_first_run() {
        let root = temp_root("stamp-write");
        provision(&root).unwrap();
        assert!(
            root.join(STAMP_FILE).exists(),
            "provision should write a bundle stamp"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn matching_stamp_skips_the_per_file_compare() {
        let root = temp_root("stamp-fast-path");
        provision(&root).unwrap();

        let main = root
            .join("Runtimes")
            .join("python-mlx-audio")
            .join("main.py");
        std::fs::remove_file(&main).unwrap();

        // The stamp still matches this build's tree, so the fast path returns
        // without re-running the per-file loop: the deleted file stays deleted.
        provision(&root).unwrap();
        assert!(
            !main.exists(),
            "a matching stamp should skip the per-file compare entirely"
        );

        // Removing the stamp forces the fallback loop, which restores the file.
        std::fs::remove_file(root.join(STAMP_FILE)).unwrap();
        provision(&root).unwrap();
        assert!(
            main.exists(),
            "an absent stamp should fall back to the per-file compare"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mismatched_stamp_triggers_the_full_compare() {
        let root = temp_root("stamp-mismatch");
        provision(&root).unwrap();

        let main = root
            .join("Runtimes")
            .join("python-mlx-audio")
            .join("main.py");
        std::fs::remove_file(&main).unwrap();
        std::fs::write(root.join(STAMP_FILE), "stale-stamp-value").unwrap();

        provision(&root).unwrap();
        assert!(
            main.exists(),
            "a mismatched stamp should fall back to the per-file compare and restore the file"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(STAMP_FILE)).unwrap(),
            bundle_stamp(),
            "a successful fallback pass should refresh the stamp to the current one"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The archive round-trip is the entire correctness guarantee of this
    /// module: the Python env setup consumes exact lockfile bytes, so a
    /// decode bug here would silently corrupt every provisioned bundle. This
    /// walks the actual source tree and asserts the decoded archive holds,
    /// byte-for-byte, exactly the same set of (path, contents) pairs.
    #[test]
    fn decoded_archive_matches_the_source_tree_byte_for_byte() {
        let runtimes_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtimes");
        let mut expected = std::collections::BTreeMap::new();
        collect_source_files(&runtimes_root, &runtimes_root, &mut expected);

        let mut decoded = std::collections::BTreeMap::new();
        for (path, contents) in decode_archive() {
            let key = path
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            decoded.insert(key, contents);
        }

        assert_eq!(
            decoded.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "the archive should contain exactly the source tree's files"
        );
        for (path, source_contents) in &expected {
            assert_eq!(
                decoded.get(path.as_str()),
                Some(source_contents),
                "{path} should decode to the exact source bytes"
            );
        }
    }

    fn collect_source_files(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_source_files(root, &path, out);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.insert(relative, std::fs::read(&path).unwrap());
        }
    }
}
