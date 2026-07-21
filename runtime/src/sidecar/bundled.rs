//! The Python runtime bundles shipped inside the binary. The Swift build carried
//! these as app resources; a headless binary has no resource bundle, so the
//! `runtime/runtimes` tree (each `python-*` bundle's `main.py`, `manifest.toml`,
//! and `requirements.*`) is embedded here and extracted into the data directory on
//! first use, where [`RuntimeBundle`](super::RuntimeBundle) then locates it.

use std::path::Path;

use include_dir::{Dir, DirEntry, include_dir};
use sha2::{Digest, Sha256};

/// The embedded `python-*` bundles, mirroring `runtime/runtimes/<name>/…`.
static RUNTIMES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/runtimes");

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

    write_entries(RUNTIMES.entries(), &bundles_root.join("Runtimes"))?;

    // Best-effort: a failed stamp write just means the next boot re-runs the
    // (correct, if slower) per-file compare instead of trusting a stale marker.
    let _ = std::fs::write(&stamp_path, &current);
    Ok(())
}

/// A hash over the embedded tree's paths and contents, so it changes whenever any
/// embedded byte does. Hashing the logical (decompressed) contents keeps this
/// stable across a future change to how the tree is stored in the binary.
fn bundle_stamp() -> String {
    let mut hasher = Sha256::new();
    absorb_entries(&mut hasher, RUNTIMES.entries());
    hex::encode(hasher.finalize())
}

/// Fold every file's path and contents into `hasher`, recursing into directories.
/// `include_dir` yields entries in a fixed build-time order, so this is stable
/// across runs of the same build.
fn absorb_entries(hasher: &mut Sha256, entries: &[DirEntry<'_>]) {
    for entry in entries {
        match entry {
            DirEntry::Dir(dir) => absorb_entries(hasher, dir.entries()),
            DirEntry::File(file) => {
                hasher.update(file.path().to_string_lossy().as_bytes());
                hasher.update([0u8]);
                hasher.update(file.contents().len().to_le_bytes());
                hasher.update(file.contents());
            }
        }
    }
}

/// Recursively write `entries` into `dest`, keyed by each file's path relative to
/// the embedded root (so `python-mlx-audio/main.py` lands at `dest/python-mlx-audio/main.py`).
fn write_entries(entries: &[DirEntry<'_>], dest: &Path) -> std::io::Result<()> {
    for entry in entries {
        match entry {
            DirEntry::Dir(dir) => write_entries(dir.entries(), dest)?,
            DirEntry::File(file) => {
                let target = dest.join(file.path());
                if is_current(&target, file.contents()) {
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
                std::fs::write(&temp, file.contents())?;
                std::fs::rename(&temp, &target)?;
            }
        }
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
}
