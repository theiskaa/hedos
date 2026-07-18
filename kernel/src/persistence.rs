//! On-disk JSON store substrate: atomic writes and corruption quarantine.
//!
//! Every persisted store in the kernel routes through these helpers so the two
//! invariants hold everywhere: writes are atomic for readers (a crash mid-write
//! cannot leave a half-written file), and an unreadable file is quarantined
//! rather than silently reset or skipped.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Errors raised by the JSON-on-disk store helpers.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A filesystem operation failed.
    #[error("store io error: {0}")]
    Io(#[from] io::Error),

    /// A value could not be serialized to JSON.
    #[error("store encode error: {0}")]
    Encode(#[source] serde_json::Error),

    /// The file existed but could not be decoded. The corruption signal is never
    /// lost: `quarantined` is `Some(path)` when the file was successfully moved
    /// aside, or `None` when quarantine itself failed (and the file was left in
    /// place). Either way the caller learns the store was corrupt.
    #[error("corrupt store at {path}: {source}")]
    Corrupt {
        /// The original path the corrupt file occupied.
        path: PathBuf,
        /// Where the corrupt file was moved, or `None` if quarantine failed.
        quarantined: Option<PathBuf>,
        /// The decode error that triggered quarantine.
        #[source]
        source: serde_json::Error,
    },
}

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

/// A suffix unique across processes and threads, used to name temp and
/// quarantine siblings so they never collide — the PID separates processes, the
/// atomic counter separates threads within a process, and the timestamp orders
/// them roughly in time.
fn unique_suffix() -> String {
    let unique = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{unique}", std::process::id(), epoch_millis())
}

fn directory_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn file_name_of(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("store")
}

/// Serialize `value` as pretty-printed JSON and write it atomically to `path`.
pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(StoreError::Encode)?;
    write_atomic(path, &bytes)?;
    Ok(())
}

/// Read and decode JSON from `path`.
///
/// Returns `Ok(None)` when the file does not exist. When the file exists but
/// cannot be decoded, it is quarantined and [`StoreError::Corrupt`] is returned —
/// the unreadable bytes are never silently discarded or overwritten. If the
/// quarantine move itself fails, the error still reports the corruption (with
/// `quarantined: None`) so the signal is never lost.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(StoreError::Io(err)),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) => Ok(Some(value)),
        Err(source) => Err(StoreError::Corrupt {
            path: path.to_path_buf(),
            quarantined: quarantine(path).ok(),
            source,
        }),
    }
}

/// Write `bytes` to `path` atomically.
///
/// The low-level primitive beneath [`write_json_atomic`]; it returns a bare
/// [`io::Result`] because no encoding is involved. Creates any missing parent
/// directories, writes a sibling temp file, flushes it, then renames it over the
/// destination. The rename is atomic on the same filesystem, so readers observe
/// either the old file or the fully written new one — never a partial write. The
/// temp file is removed on every failure path, and the parent directory is
/// flushed after a successful rename for durability (best-effort).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = directory_of(path);
    fs::create_dir_all(&directory)?;
    let temp = temp_sibling(path);
    if let Err(err) = write_temp_then_rename(&temp, path, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    let _ = File::open(&directory).and_then(|dir| dir.sync_all());
    Ok(())
}

fn write_temp_then_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    {
        let mut file = File::create(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(temp, path)
}

/// Rename a corrupt file to a sibling `<name>.corrupt-<pid>-<epoch-millis>-<n>`
/// and return the new path. The low-level primitive beneath [`read_json`]; it
/// returns a bare [`io::Result`] and leaves the original path free for a fresh
/// default to take.
pub fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let target = directory_of(path).join(format!(
        "{}.corrupt-{}",
        file_name_of(path),
        unique_suffix()
    ));
    fs::rename(path, &target)?;
    Ok(target)
}

fn temp_sibling(path: &Path) -> PathBuf {
    directory_of(path).join(format!(".{}.tmp-{}", file_name_of(path), unique_suffix()))
}
