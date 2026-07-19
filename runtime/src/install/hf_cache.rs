//! Writing a Hugging Face model into the on-disk hub cache — the same
//! `models--<org>--<name>/{blobs,snapshots/<rev>,refs}` layout the discovery
//! scanners read. Downloads are resumable (range requests over a saved
//! `.incomplete` blob) and hash-verified, and blobs are content-addressed by
//! their LFS SHA-256 so repeated installs dedup.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use kernel::install::InstallError;
use kernel::install::file_selection::HFSibling;
use sha2::{Digest, Sha256};

use super::transport::{InstallRequest, InstallTransport};

/// Read the resume hash in 1 MiB slices.
const HASH_CHUNK_BYTES: usize = 1 << 20;
/// Stray `.incomplete` blobs younger than this are left alone (another install
/// may still be writing them); older ones are reaped.
const STALE_INCOMPLETE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The on-disk paths of one repo's hub-cache directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HFCacheLayout {
    root: PathBuf,
    repo: String,
}

impl HFCacheLayout {
    /// The layout for `repo` rooted at the hub-cache `root`.
    pub fn new(root: impl Into<PathBuf>, repo: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            repo: repo.into(),
        }
    }

    /// The `models--<org>--<name>` directory for this repo.
    pub fn repo_directory(&self) -> PathBuf {
        self.root
            .join(format!("models--{}", self.repo.replace('/', "--")))
    }

    fn blobs_directory(&self) -> PathBuf {
        self.repo_directory().join("blobs")
    }

    fn refs_directory(&self) -> PathBuf {
        self.repo_directory().join("refs")
    }

    fn snapshot_directory(&self, revision: &str) -> PathBuf {
        self.repo_directory().join("snapshots").join(revision)
    }

    fn snapshot_file(&self, revision: &str, path: &str) -> PathBuf {
        let mut file = self.snapshot_directory(revision);
        for component in path.split('/').filter(|part| !part.is_empty()) {
            file.push(component);
        }
        file
    }

    fn blob_url(&self, name: &str) -> PathBuf {
        self.blobs_directory().join(name)
    }

    fn incomplete_url(&self, name: &str) -> PathBuf {
        self.blobs_directory().join(format!("{name}.incomplete"))
    }

    /// The relative symlink target from a snapshot file back to its blob. Empty
    /// path segments are ignored so the depth matches where the snapshot file
    /// actually lands (empty segments are dropped).
    fn relative_blob_target(path: &str, blob_name: &str) -> String {
        let depth = path.split('/').filter(|part| !part.is_empty()).count() + 1;
        format!("{}blobs/{blob_name}", "../".repeat(depth))
    }
}

/// Downloads a repo's files into an [`HFCacheLayout`].
pub struct HFCacheWriter {
    layout: HFCacheLayout,
    transport: Arc<dyn InstallTransport>,
}

impl HFCacheWriter {
    /// A writer for `layout`, fetching over `transport`.
    pub fn new(layout: HFCacheLayout, transport: Arc<dyn InstallTransport>) -> Self {
        Self { layout, transport }
    }

    /// The layout this writer targets.
    pub fn layout(&self) -> &HFCacheLayout {
        &self.layout
    }

    /// The blob name a not-yet-downloaded file will use: its LFS SHA-256 when
    /// known, else a deterministic `tmp-…` name derived from revision + path.
    pub fn pending_blob_name(sibling: &HFSibling, revision: &str) -> String {
        if let Some(sha) = &sibling.sha256 {
            return sha.clone();
        }
        let mut hasher = Sha256::new();
        hasher.update(format!("{revision}\n{}", sibling.rfilename).as_bytes());
        format!("tmp-{}", hex(&hasher.finalize()))
    }

    /// Bytes of `selection` already on disk at `revision`: a completed
    /// content-addressed blob counts its full size; otherwise the saved
    /// `.incomplete`'s current length. Drives a plan's remaining-bytes estimate.
    pub fn present_bytes(&self, selection: &[HFSibling], revision: &str) -> i64 {
        selection.iter().fold(0i64, |present, sibling| {
            if let Some(sha) = &sibling.sha256
                && self.layout.blob_url(sha).exists()
            {
                return present.saturating_add(sibling.bytes.unwrap_or(0).max(0));
            }
            let pending = self
                .layout
                .incomplete_url(&Self::pending_blob_name(sibling, revision));
            let size = fs::metadata(&pending)
                .map(|meta| meta.len() as i64)
                .unwrap_or(0);
            present.saturating_add(size.max(0))
        })
    }

    /// Create the `blobs`/`snapshots/<rev>`/`refs` skeleton, write `refs/main` if
    /// absent, and lay down a placeholder `.incomplete` for the first weight so an
    /// interrupted install is recognizably in-progress.
    pub fn prepare_skeleton(
        &self,
        revision: &str,
        first_weight_pending_name: Option<&str>,
    ) -> Result<(), InstallError> {
        fs::create_dir_all(self.layout.blobs_directory()).map_err(io_err)?;
        fs::create_dir_all(self.layout.snapshot_directory(revision)).map_err(io_err)?;
        fs::create_dir_all(self.layout.refs_directory()).map_err(io_err)?;
        let reference = self.layout.refs_directory().join("main");
        if !reference.exists() {
            write_atomic(&reference, revision.as_bytes())?;
        }
        if let Some(name) = first_weight_pending_name {
            let pending = self.layout.incomplete_url(name);
            if !pending.exists() && !self.layout.blob_url(name).exists() {
                fs::File::create(&pending).map_err(io_err)?;
            }
        }
        Ok(())
    }

    /// Download `sibling`, resuming from any saved `.incomplete`, verifying the
    /// hash, moving it into `blobs/`, and symlinking it under `snapshots/<rev>`.
    /// `on_bytes` receives each byte delta (negative when a partial is discarded).
    pub async fn download(
        &self,
        sibling: &HFSibling,
        revision: &str,
        request: InstallRequest,
        on_bytes: &mut (dyn FnMut(i64) + Send),
    ) -> Result<(), InstallError> {
        let pending_name = Self::pending_blob_name(sibling, revision);

        // Already have the content-addressed blob — just link it.
        if let Some(sha) = &sibling.sha256
            && self.layout.blob_url(sha).exists()
        {
            let _ = fs::remove_file(self.layout.incomplete_url(&pending_name));
            self.link(&sibling.rfilename, revision, sha)?;
            on_bytes(sibling.bytes.unwrap_or(0));
            return Ok(());
        }

        let incomplete = self.layout.incomplete_url(&pending_name);
        let mut hasher = Sha256::new();
        let mut written: i64 = 0;
        if incomplete.exists() {
            written = self.hash_existing(&incomplete, &mut hasher)?;
            if written > 0 {
                on_bytes(written);
            }
        } else {
            fs::File::create(&incomplete).map_err(io_err)?;
        }

        let base_request = request.clone();
        let request = if written > 0 {
            request.header("Range", format!("bytes={written}-"))
        } else {
            request
        };
        let mut start = self.transport.stream(request).await?;

        // The saved partial is past the end — start over from scratch.
        if start.status == 416 && written > 0 {
            fs::write(&incomplete, b"").map_err(io_err)?;
            hasher = Sha256::new();
            on_bytes(-written);
            written = 0;
            start = self.transport.stream(base_request).await?;
        }

        match start.status {
            200 => {
                // A full response despite our range — discard the partial.
                if written > 0 {
                    fs::write(&incomplete, b"").map_err(io_err)?;
                    hasher = Sha256::new();
                    on_bytes(-written);
                    written = 0;
                }
            }
            206 => {}
            401 | 403 => return Err(InstallError::AuthRequired(self.layout.repo.clone())),
            404 => {
                return Err(InstallError::TransferFailed(format!(
                    "{} is missing from {}",
                    sibling.rfilename, self.layout.repo
                )));
            }
            other => {
                return Err(InstallError::TransferFailed(format!(
                    "hugging face returned HTTP {other} for {}",
                    sibling.rfilename
                )));
            }
        }

        let mut handle = fs::OpenOptions::new()
            .append(true)
            .open(&incomplete)
            .map_err(io_err)?;
        while let Some(chunk) = start.chunks.recv().await {
            let chunk = chunk?;
            handle.write_all(&chunk).map_err(io_err)?;
            hasher.update(&chunk);
            written += chunk.len() as i64;
            on_bytes(chunk.len() as i64);
        }
        handle.flush().map_err(io_err)?;
        drop(handle);

        if let Some(expected) = sibling.bytes
            && written != expected
        {
            return Err(InstallError::TransferFailed(format!(
                "{} ended after {written} of {expected} bytes",
                sibling.rfilename
            )));
        }

        let digest = hex(&hasher.finalize());
        if let Some(sha) = &sibling.sha256
            && &digest != sha
        {
            let _ = fs::remove_file(&incomplete);
            return Err(InstallError::ChecksumMismatch(sibling.rfilename.clone()));
        }

        let final_name = sibling.sha256.clone().unwrap_or(digest);
        let blob = self.layout.blob_url(&final_name);
        if blob.exists() {
            fs::remove_file(&incomplete).map_err(io_err)?;
        } else {
            fs::rename(&incomplete, &blob).map_err(io_err)?;
        }
        self.link(&sibling.rfilename, revision, &final_name)?;
        Ok(())
    }

    /// Write `refs/main`, marking the revision fully present.
    pub fn commit_ref(&self, revision: &str) -> Result<(), InstallError> {
        write_atomic(
            &self.layout.refs_directory().join("main"),
            revision.as_bytes(),
        )
    }

    /// Remove `.incomplete` blobs that aren't in `keeping` and are older than the
    /// stale cutoff (a fresh one may belong to a concurrent install).
    pub fn remove_stray_incompletes(&self, keeping: &HashSet<String>) -> Result<(), InstallError> {
        let cutoff = SystemTime::now()
            .checked_sub(STALE_INCOMPLETE_AGE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.reap_incompletes(keeping, cutoff)
    }

    /// The reaping core with an explicit cutoff (the seam tests drive). A missing
    /// blobs dir is fine (nothing to reap); other read errors surface.
    fn reap_incompletes(
        &self,
        keeping: &HashSet<String>,
        cutoff: SystemTime,
    ) -> Result<(), InstallError> {
        let entries = match fs::read_dir(self.layout.blobs_directory()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_err(error)),
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(base) = name.strip_suffix(".incomplete") else {
                continue;
            };
            if keeping.contains(base) {
                continue;
            }
            if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified())
                && modified > cutoff
            {
                continue;
            }
            let _ = fs::remove_file(entry.path());
        }
        Ok(())
    }

    /// Delete the whole repo directory.
    pub fn remove_repo(&self) {
        let _ = fs::remove_dir_all(self.layout.repo_directory());
    }

    /// Whether any blob is at least `minimum_bytes` — i.e. a real download landed.
    pub fn has_substantial_progress(&self, minimum_bytes: i64) -> bool {
        let Ok(entries) = fs::read_dir(self.layout.blobs_directory()) else {
            return false;
        };
        entries.flatten().any(|entry| {
            entry
                .metadata()
                .map(|meta| meta.len() as i64 >= minimum_bytes)
                .unwrap_or(false)
        })
    }

    /// Whether any completed (non-`.incomplete`) blob exists.
    pub fn has_completed_blob(&self) -> bool {
        let Ok(entries) = fs::read_dir(self.layout.blobs_directory()) else {
            return false;
        };
        entries
            .flatten()
            .any(|entry| !entry.file_name().to_string_lossy().ends_with(".incomplete"))
    }

    /// Drop the ref and snapshot trees, leaving only the blobs — used when an
    /// interrupted install saved bytes but no blob is complete yet.
    pub fn retreat_to_blobs_only(&self) {
        let _ = fs::remove_dir_all(self.layout.refs_directory());
        let _ = fs::remove_dir_all(self.layout.repo_directory().join("snapshots"));
    }

    fn link(&self, path: &str, revision: &str, blob_name: &str) -> Result<(), InstallError> {
        let destination = self.layout.snapshot_file(revision, path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        // Replace any existing file, (possibly dangling) symlink, or stray dir.
        if fs::symlink_metadata(&destination).is_ok() && fs::remove_file(&destination).is_err() {
            let _ = fs::remove_dir_all(&destination);
        }
        let target = HFCacheLayout::relative_blob_target(path, blob_name);
        symlink(&target, &destination)
    }

    fn hash_existing(&self, path: &Path, hasher: &mut Sha256) -> Result<i64, InstallError> {
        let mut file = fs::File::open(path).map_err(io_err)?;
        let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
        let mut total: i64 = 0;
        loop {
            let read = file.read(&mut buffer).map_err(io_err)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            total += read as i64;
        }
        Ok(total)
    }
}

fn io_err(error: std::io::Error) -> InstallError {
    InstallError::TransferFailed(error.to_string())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A per-process counter so concurrent `write_atomic` calls (even for the same
/// path) use distinct temp files.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `data` to `path` via a uniquely-named temp file + rename, so a reader
/// never sees a half-written file and concurrent writers don't clobber the temp.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), InstallError> {
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}-{unique}", std::process::id()));
    let temp = path.with_file_name(name);
    fs::write(&temp, data).map_err(io_err)?;
    fs::rename(&temp, path).map_err(io_err)
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<(), InstallError> {
    std::os::unix::fs::symlink(target, link).map_err(io_err)
}

#[cfg(not(unix))]
fn symlink(target: &str, link: &Path) -> Result<(), InstallError> {
    // No symlinks: resolve the relative target and copy the blob in its place.
    let source = link
        .parent()
        .map(|parent| parent.join(target))
        .unwrap_or_else(|| PathBuf::from(target));
    fs::copy(&source, link).map(|_| ()).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoTransport;
    impl InstallTransport for NoTransport {
        fn fetch(&self, _request: InstallRequest) -> super::super::transport::TransportFuture {
            Box::pin(async { Err(InstallError::TransferFailed("unused".to_owned())) })
        }
    }

    fn temp_writer() -> (PathBuf, HFCacheWriter) {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("hedos-hfcache-unit-{stamp}-{counter}"));
        let writer = HFCacheWriter::new(
            HFCacheLayout::new(&root, "org/Model"),
            Arc::new(NoTransport),
        );
        (root, writer)
    }

    #[test]
    fn pending_blob_name_uses_the_sha_else_a_deterministic_tmp_name() {
        let with_sha = HFSibling::new("w.bin", Some(1)).with_sha256(Some("abc123".to_owned()));
        assert_eq!(HFCacheWriter::pending_blob_name(&with_sha, "rev"), "abc123");

        let plain = HFSibling::new("w.bin", Some(1));
        let name = HFCacheWriter::pending_blob_name(&plain, "rev");
        assert!(name.starts_with("tmp-"));
        // Deterministic in revision + path.
        assert_eq!(name, HFCacheWriter::pending_blob_name(&plain, "rev"));
        assert_ne!(name, HFCacheWriter::pending_blob_name(&plain, "other-rev"));
    }

    #[test]
    fn reap_removes_stale_unkept_incompletes_but_spares_kept_ones() {
        let (root, writer) = temp_writer();
        writer.prepare_skeleton("rev", None).expect("skeleton");
        let blobs = writer.layout.blobs_directory();
        fs::write(blobs.join("keepme.incomplete"), b"x").unwrap();
        fs::write(blobs.join("stale.incomplete"), b"y").unwrap();
        fs::write(blobs.join("realblob"), b"z").unwrap();

        // A cutoff in the future makes every file "older than the cutoff" → reapable.
        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        let mut keeping = HashSet::new();
        keeping.insert("keepme".to_owned());
        writer.reap_incompletes(&keeping, cutoff).expect("reap");

        assert!(blobs.join("keepme.incomplete").exists(), "kept name spared");
        assert!(!blobs.join("stale.incomplete").exists(), "stale reaped");
        assert!(blobs.join("realblob").exists(), "non-incomplete untouched");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reap_is_a_noop_when_the_blobs_dir_is_absent() {
        let (root, writer) = temp_writer();
        // No skeleton → no blobs dir.
        assert!(
            writer
                .reap_incompletes(&HashSet::new(), SystemTime::now())
                .is_ok()
        );
        fs::remove_dir_all(&root).ok();
    }
}
