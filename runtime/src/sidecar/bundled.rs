//! The Python runtime bundles shipped inside the binary. The Swift build carried
//! these as app resources; a headless binary has no resource bundle, so the
//! `runtime/runtimes` tree (each `python-*` bundle's `main.py`, `manifest.toml`,
//! and `requirements.*`) is embedded here and extracted into the data directory on
//! first use, where [`RuntimeBundle`](super::RuntimeBundle) then locates it.

use std::path::Path;

use include_dir::{Dir, DirEntry, include_dir};

/// The embedded `python-*` bundles, mirroring `runtime/runtimes/<name>/…`.
static RUNTIMES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/runtimes");

/// Extract every embedded bundle under `bundles_root/Runtimes`, writing only files
/// that are missing or whose contents differ, so a shipped update refreshes the
/// bundle while leaving an up-to-date tree (and its mtimes) untouched.
///
/// Best-effort and idempotent: the caller runs it on every boot. A failure leaves
/// the bundle absent, which surfaces later as the usual "runtime bundle is missing".
pub fn provision(bundles_root: &Path) -> std::io::Result<()> {
    write_entries(RUNTIMES.entries(), &bundles_root.join("Runtimes"))
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

        // A local edit is overwritten back to the shipped contents on the next run.
        std::fs::write(&main, b"tampered").unwrap();
        provision(&root).unwrap();
        assert_eq!(std::fs::read(&main).unwrap(), shipped);

        std::fs::remove_dir_all(&root).ok();
    }
}
