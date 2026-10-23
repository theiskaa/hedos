//! What installing an engine on request takes, whichever engine it is: the
//! lock one install or removal holds on the engine's directory, the staging
//! directory an install unpacks into and what an interrupted one leaves,
//! fetching an archive checked against its pinned size and sha256, unpacking
//! it with nothing reaching outside its directory, asking what it unpacked
//! its version, and moving it into place.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use fs2::FileExt;
use kernel::engines::{INSTALLED_FILE, InstalledFile};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Why an engine could not be installed or removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineInstallError {
    /// The pinned release has no build of this variant for this machine.
    #[error("llama.cpp {version} has no {variant} build for {os} {arch}")]
    NoBuild {
        /// The release's version.
        version: String,
        /// The variant asked for.
        variant: String,
        /// The operating system.
        os: String,
        /// The CPU architecture.
        arch: String,
    },
    /// The pinned release of an engine has no build for this machine.
    #[error("{engine} {version} has no build for {os} {arch}")]
    Unsupported {
        /// The engine.
        engine: String,
        /// The release's version.
        version: String,
        /// The operating system.
        os: String,
        /// The CPU architecture.
        arch: String,
    },
    /// Another install or removal holds the lock.
    #[error("another {engine} install is running; try again when it finishes")]
    Busy {
        /// The engine being installed or removed.
        engine: String,
    },
    /// The build would not start on this system, so nothing is fetched.
    #[error("{reason}")]
    WontRun {
        /// Why, and what runs instead.
        reason: String,
    },
    /// An archive could not be fetched.
    #[error("downloading {url}: {reason}")]
    Download {
        /// What was fetched.
        url: String,
        /// Why it failed.
        reason: String,
    },
    /// An archive is not the one the release pins.
    #[error("{url} is not the archive hedos pins (sha256 {found}, expected {expected})")]
    Checksum {
        /// What was fetched.
        url: String,
        /// The pinned digest.
        expected: String,
        /// The digest of what arrived.
        found: String,
    },
    /// An archive holds something hedos will not unpack.
    #[error("{archive} holds {entry}, which hedos does not unpack: {reason}")]
    Archive {
        /// The archive's file name.
        archive: String,
        /// The entry.
        entry: String,
        /// Why it was refused.
        reason: String,
    },
    /// The unpacked program is not the release pinned.
    #[error("the unpacked {program} is not {expected}: {found}")]
    Smoke {
        /// The program run (`llama-server`, `uv`).
        program: String,
        /// The release expected (`llama.cpp b11669`, `uv 0.13.4`).
        expected: String,
        /// What it reported, or that it reported nothing.
        found: String,
    },
    /// The install was dropped (Ctrl-C, or the shelf quitting) before what
    /// it unpacked was put in place, so nothing was.
    #[error("the {engine} install was stopped")]
    Stopped {
        /// The engine being installed.
        engine: String,
    },
    /// The new build could not be put in place, and the one it replaced
    /// could not be put back either.
    #[error(
        "{reason}; the build hedos had is kept at {kept}, and the next install or removal puts it back"
    )]
    Stranded {
        /// Where the build it replaced is kept.
        kept: String,
        /// Why the new one could not be put in place.
        reason: String,
    },
    /// Reading or writing a file failed.
    #[error("{path}: {reason}")]
    Io {
        /// The file.
        path: String,
        /// Why it failed.
        reason: String,
    },
}

/// How far an install has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProgress {
    /// Bytes of `file` have arrived, out of `total` across every archive.
    Downloading {
        /// The archive being fetched.
        file: String,
        /// Bytes received so far, across every archive.
        received: u64,
        /// Bytes to receive in all.
        total: u64,
    },
    /// The archives arrived and are being unpacked.
    Unpacking,
    /// The unpacked program is being asked its version.
    Checking,
}

/// How long connecting to the release host may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the download may go without a byte before it is given up on.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// The lock file, in the directory hedos keeps an engine's installs in, held
/// while one install or removal of that engine runs.
pub(crate) const LOCK_FILE: &str = ".lock";

/// How long taking the install lock is retried before an install is taken to
/// be running.
const LOCK_PATIENCE: Duration = Duration::from_secs(1);

/// How often it is retried meanwhile.
const LOCK_RETRY: Duration = Duration::from_millis(20);

/// How long an unpacked program may take to say its version.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of each stream a program asked its version writes that is read.
const VERSION_READ: u64 = 64 * 1024;

/// How much of what a failing program wrote an error carries.
const SAID: usize = 600;

/// The prefix of a staging directory an install unpacks into.
pub(crate) const STAGING_PREFIX: &str = ".staging-";

/// Remove the installs under `root`: every entry but the lock and staging
/// directories, a link removed as a link rather than followed.
pub(crate) fn remove_installs(root: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
    let mut removed = Vec::new();
    for path in installs(root)? {
        remove_entry(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

/// Remove `path`: a directory with what is in it, anything else (a link
/// included) as itself.
pub(crate) fn remove_entry(path: &Path) -> Result<(), EngineInstallError> {
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    if is_dir {
        std::fs::remove_dir_all(path).map_err(|error| io_error(path, error))
    } else {
        std::fs::remove_file(path).map_err(|error| io_error(path, error))
    }
}

/// What is installed under `root`: every entry but hidden ones (the lock and
/// staging directories).
pub(crate) fn installs(root: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
    let entries = std::fs::read_dir(root).map_err(|error| io_error(root, error))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error(root, error))?;
        if !entry.file_name().to_string_lossy().starts_with('.') {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// Finish what an install that did not end left in `root`, run under the
/// lock, so no install is running. A build one set aside goes back when
/// nothing of its name took its place, and is dropped when something did;
/// then the staging directory goes. One holding a build that cannot be put
/// back is kept and reported, so nothing goes on as if it were finished.
pub(crate) fn recover(root: &Path) -> Result<(), EngineInstallError> {
    let entries = std::fs::read_dir(root).map_err(|error| io_error(root, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(root, error))?;
        let is_staging = entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !is_staging {
            continue;
        }
        let staging = entry.path();
        let aside = staging.join("replaced");
        let mut kept = false;
        if aside.is_dir() {
            for build in std::fs::read_dir(&aside).map_err(|error| io_error(&aside, error))? {
                let build = build.map_err(|error| io_error(&aside, error))?;
                let home = root.join(build.file_name());
                if std::fs::symlink_metadata(&home).is_err() {
                    kept |= std::fs::rename(build.path(), &home).is_err();
                }
            }
        }
        if kept {
            return Err(EngineInstallError::Stranded {
                kept: aside.to_string_lossy().into_owned(),
                reason: "a build an earlier install set aside could not be put back".to_owned(),
            });
        }
        std::fs::remove_dir_all(&staging).map_err(|error| io_error(&staging, error))?;
    }
    Ok(())
}

/// Fetch `url` into `path`, refusing more than `bytes` and anything whose
/// sha256 is not `sha256`. Returns `received` plus what arrived.
pub(crate) async fn download(
    url: &str,
    path: &Path,
    sha256: &str,
    bytes: u64,
    mut received: u64,
    total: u64,
    progress: &impl Fn(InstallProgress),
) -> Result<u64, EngineInstallError> {
    let failed = |reason: String| EngineInstallError::Download {
        url: url.to_owned(),
        reason,
    };
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(IDLE_TIMEOUT)
        .build()
        .map_err(|error| failed(error.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| failed(error.to_string()))?;
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| io_error(path, error))?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| failed(error.to_string()))?
    {
        written += chunk.len() as u64;
        if written > bytes {
            return Err(failed(format!(
                "it is larger than the {bytes} bytes pinned"
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| io_error(path, error))?;
        received += chunk.len() as u64;
        progress(InstallProgress::Downloading {
            file: file_name.clone(),
            received,
            total,
        });
    }
    file.flush().await.map_err(|error| io_error(path, error))?;
    let found = hex::encode(hasher.finalize());
    if found != sha256 {
        return Err(EngineInstallError::Checksum {
            url: url.to_owned(),
            expected: sha256.to_owned(),
            found,
        });
    }
    Ok(received)
}

/// Unpack the tar `archive` (already decompressed) into `target`, dropping
/// its top-level directory (`llama-b11669/llama-server` lands as
/// `target/llama-server`).
/// Only plain files, directories and symlinks within `target` are written:
/// an absolute path, a `..`, a link that leads out, or any other kind of entry
/// refuses the whole archive.
pub(crate) fn unpack(
    archive: impl Read,
    name: &str,
    target: &Path,
) -> Result<(), EngineInstallError> {
    let refused = |entry: &str, reason: &str| EngineInstallError::Archive {
        archive: name.to_owned(),
        entry: entry.to_owned(),
        reason: reason.to_owned(),
    };
    create_dir_all(target)?;
    let mut entries = tar::Archive::new(archive);
    for entry in entries
        .entries()
        .map_err(|error| refused("the archive", &error.to_string()))?
    {
        let mut entry = entry.map_err(|error| refused("an entry", &error.to_string()))?;
        let inside = entry
            .path()
            .map_err(|error| refused("an entry", &error.to_string()))?
            .into_owned();
        let shown = inside.to_string_lossy().into_owned();
        let Some(relative) = within(&inside) else {
            return Err(refused(&shown, "its path leaves the archive"));
        };
        let Some(relative) = relative else {
            continue;
        };
        let destination = target.join(&relative);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            create_dir_all(&destination)?;
        } else if kind.is_file() {
            if let Some(parent) = destination.parent() {
                create_dir_all(parent)?;
            }
            let mode = entry.header().mode().unwrap_or(0o644) & 0o755;
            let mut out =
                File::create(&destination).map_err(|error| io_error(&destination, error))?;
            let mut buffer = [0u8; 1 << 16];
            loop {
                let read = entry
                    .read(&mut buffer)
                    .map_err(|error| refused(&shown, &error.to_string()))?;
                if read == 0 {
                    break;
                }
                out.write_all(&buffer[..read])
                    .map_err(|error| io_error(&destination, error))?;
            }
            set_mode(&destination, mode)?;
        } else if kind.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|error| refused(&shown, &error.to_string()))?
                .ok_or_else(|| refused(&shown, "the link names nothing"))?
                .into_owned();
            // A build links a library to its versioned file beside it (llama.cpp
            // does); a link anywhere else could lead out of the build.
            let beside = link.components().count() == 1
                && matches!(link.components().next(), Some(Component::Normal(_)));
            if !beside {
                return Err(refused(&shown, "the link leads out of its directory"));
            }
            if let Some(parent) = destination.parent() {
                create_dir_all(parent)?;
            }
            std::os::unix::fs::symlink(&link, &destination)
                .map_err(|error| io_error(&destination, error))?;
        } else {
            return Err(refused(&shown, "it is not a file, a directory or a link"));
        }
    }
    Ok(())
}

/// `inside`, an archive entry's path, with its top-level directory dropped:
/// `None` when it is absolute or climbs with `..`, `Some(None)` for the
/// top-level directory itself.
fn within(inside: &Path) -> Option<Option<PathBuf>> {
    let mut parts = Vec::new();
    for part in inside.components() {
        match part {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    let kept: PathBuf = if parts.len() > 1 {
        parts[1..].iter().collect()
    } else {
        // A file at the archive's root has no directory to drop.
        match parts.first() {
            Some(only) if !inside.to_string_lossy().ends_with('/') => PathBuf::from(only),
            _ => return Some(None),
        }
    };
    Some(Some(kept))
}

fn set_mode(path: &Path, mode: u32) -> Result<(), EngineInstallError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| io_error(path, error))
}

pub(crate) fn create_dir_all(path: &Path) -> Result<(), EngineInstallError> {
    std::fs::create_dir_all(path).map_err(|error| io_error(path, error))
}

/// The install lock on `root`, hedos's directory of `engine`, taken without
/// waiting on an install: a second install while one runs is refused rather
/// than queued behind a long download. It is tried for up to `LOCK_PATIENCE`, since a process this one
/// is starting holds a copy of every open file, the lock among them, until it
/// execs, and that must not read as an install running.
pub(crate) fn lock(root: &Path, engine: &str) -> Result<File, EngineInstallError> {
    lock_within(root, engine, LOCK_PATIENCE)
}

/// The lock file at `path`, opened as it is, never truncated, never through a
/// link and never waiting on a pipe, and kept only when what opened is a plain
/// file.
fn open_lock(path: &Path) -> Result<File, EngineInstallError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| io_error(path, error))?;
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return Err(io_error(path, "not a plain file"));
    }
    Ok(file)
}

/// The install lock on `root` if nothing holds it, without waiting at all.
pub(crate) fn try_lock(root: &Path) -> Result<Option<File>, EngineInstallError> {
    let file = open_lock(&root.join(LOCK_FILE))?;
    Ok(file.try_lock_exclusive().is_ok().then_some(file))
}

/// The install lock on `root`, waited for up to `wait`: an unpack of the build
/// hedos carries that another process is doing takes seconds, and is waited
/// out; an install on request downloading can take minutes, and is refused
/// as [`EngineInstallError::Busy`] once `wait` runs out.
pub(crate) fn lock_within(
    root: &Path,
    engine: &str,
    wait: Duration,
) -> Result<File, EngineInstallError> {
    let file = open_lock(&root.join(LOCK_FILE))?;
    let started = Instant::now();
    while file.try_lock_exclusive().is_err() {
        if started.elapsed() >= wait {
            return Err(EngineInstallError::Busy {
                engine: engine.to_owned(),
            });
        }
        std::thread::sleep(LOCK_RETRY);
    }
    Ok(file)
}

/// A new, empty staging directory under `root`, named for this process and
/// moment so no earlier one is reused.
pub(crate) fn fresh_staging(root: &Path) -> Result<PathBuf, EngineInstallError> {
    let staging = root.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        kernel::time::now_millis()
    ));
    std::fs::create_dir(&staging).map_err(|error| io_error(&staging, error))?;
    Ok(staging)
}

pub(crate) fn io_error(path: &Path, error: impl std::fmt::Display) -> EngineInstallError {
    EngineInstallError::Io {
        path: path.to_string_lossy().into_owned(),
        reason: error.to_string(),
    }
}

/// Every file directly in `dir`, with its size, or none for a link.
pub(crate) fn inventory(dir: &Path) -> Result<Vec<InstalledFile>, EngineInstallError> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|error| io_error(dir, error))? {
        let entry = entry.map_err(|error| io_error(dir, error))?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        let bytes = if meta.file_type().is_symlink() {
            None
        } else if meta.is_file() {
            Some(meta.len())
        } else {
            continue;
        };
        files.push(InstalledFile {
            name: entry.file_name().to_string_lossy().into_owned(),
            bytes,
        });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// What an unpacked program wrote when asked its `--version`, and how it
/// exited.
pub(crate) struct Said {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Said {
    /// Why its answer was not taken: how it exited and the end of what it
    /// wrote, its errors when it wrote any.
    pub(crate) fn failure(&self) -> String {
        let written = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let tail: String = written.trim().chars().rev().take(SAID).collect();
        let tail: String = tail.chars().rev().collect();
        format!("it exited with {} and said: {tail}", self.status)
    }
}

/// Run `program --version` and read what it writes, or say why it could not
/// be asked: it would not start, or did not answer in time. Its output goes to
/// files in `staging`, not pipes, so nothing is left waiting on a pipe some
/// process it started still holds.
pub(crate) fn ask_version(program: &Path, staging: &Path) -> Result<Said, String> {
    let (out, err) = (staging.join("version.out"), staging.join("version.err"));
    let create =
        |path: &Path| File::create(path).map_err(|error| format!("{}: {error}", path.display()));
    let mut child = crate::process::spawn_written(
        Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::from(create(&out)?))
            .stderr(Stdio::from(create(&err)?)),
    )
    .map_err(|error| format!("it could not start: {error}"))?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not answer within {} s",
                    VERSION_TIMEOUT.as_secs()
                ));
            }
        }
    };
    let read = |path: &Path| {
        let mut said = Vec::new();
        if let Ok(file) = File::open(path) {
            let _ = file.take(VERSION_READ).read_to_end(&mut said);
        }
        String::from_utf8_lossy(&said).into_owned()
    };
    Ok(Said {
        status,
        stdout: read(&out),
        stderr: read(&err),
    })
}

/// Where an install stands for the hand-off between [`Dropped`] and
/// [`Commit`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Stage {
    /// Still going: nothing put in place, nothing dropped.
    #[default]
    Going,
    /// Dropped before anything was put in place, so nothing will be.
    Dropped,
    /// Putting what it unpacked in place, which a drop then waits out.
    Committing,
    /// What it unpacked is in place, here.
    Landed(PathBuf),
}

/// What became of an install, for its caller to read after dropping it
/// (Ctrl-C): where it put what it unpacked, if it got that far. The install
/// itself records it, so nothing older is taken for it.
#[derive(Debug, Clone, Default)]
pub struct Landing(Arc<Mutex<Stage>>);

impl Landing {
    /// The directory the install put in place, once it has.
    pub fn landed(&self) -> Option<PathBuf> {
        match &*self.0.lock().unwrap_or_else(PoisonError::into_inner) {
            Stage::Landed(dir) => Some(dir.clone()),
            _ => None,
        }
    }
}

/// The install future's side of its hand-off with the blocking step that
/// puts what it unpacked in place, a step that cannot be stopped once
/// started. Dropping it (Ctrl-C, or the shelf quitting) before that step
/// begins means nothing is put in place; dropping it during the step waits
/// for the step to end, so the caller's [`Landing`] then says whether it
/// landed.
pub(crate) struct Dropped(Landing);

impl Dropped {
    /// Watch the install `landing` reports on.
    pub(crate) fn watching(landing: &Landing) -> Self {
        Dropped(landing.clone())
    }

    /// The blocking step's side.
    pub(crate) fn commit(&self) -> Commit {
        Commit(Some(self.0.clone()))
    }
}

impl Drop for Dropped {
    fn drop(&mut self) {
        let mut stage = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        if *stage == Stage::Going {
            *stage = Stage::Dropped;
        }
    }
}

/// The blocking step's side of [`Dropped`].
pub(crate) struct Commit(Option<Landing>);

impl Commit {
    /// For an install nothing drops midway (unpacking the carried build).
    pub(crate) fn undroppable() -> Self {
        Commit(None)
    }

    /// Run `commit`, which puts a directory in place and says where, unless
    /// the install was dropped before it, which is
    /// [`EngineInstallError::Stopped`]; a drop meanwhile waits for it, and
    /// where it landed is recorded for the caller.
    pub(crate) fn run(
        &self,
        engine: &str,
        commit: impl FnOnce() -> Result<PathBuf, EngineInstallError>,
    ) -> Result<PathBuf, EngineInstallError> {
        let Some(landing) = &self.0 else {
            return commit();
        };
        let mut stage = landing.0.lock().unwrap_or_else(PoisonError::into_inner);
        if *stage == Stage::Dropped {
            return Err(EngineInstallError::Stopped {
                engine: engine.to_owned(),
            });
        }
        *stage = Stage::Committing;
        let result = commit();
        if let Ok(dir) = &result {
            *stage = Stage::Landed(dir.clone());
        }
        result
    }
}

/// Where an install on request works: the engine's directory, its staging
/// directory, the lock it holds, and where it says it landed.
#[derive(Clone, Copy)]
pub(crate) struct Site<'a> {
    pub(crate) root: &'a Path,
    pub(crate) staging: &'a Path,
    pub(crate) lock: &'a Arc<File>,
    pub(crate) landing: &'a Landing,
}

/// Write `record` beside what is unpacked at `unpacked`, then move it to
/// `root/name`: what is there already is set aside in `staging` first, and
/// put back if the new one cannot take its place. The moves are `commit`'s:
/// none happens once the install was dropped, and a drop during them waits.
pub(crate) fn put_in_place(
    record: &impl Serialize,
    engine: &str,
    unpacked: &Path,
    root: &Path,
    staging: &Path,
    name: &str,
    commit: &Commit,
) -> Result<PathBuf, EngineInstallError> {
    let encoded = serde_json::to_vec_pretty(record).map_err(|error| io_error(unpacked, error))?;
    let record_path = unpacked.join(INSTALLED_FILE);
    std::fs::write(&record_path, encoded).map_err(|error| io_error(&record_path, error))?;
    commit.run(engine, || {
        let destination = root.join(name);
        let aside = staging.join("replaced");
        let set_aside = aside.join(name);
        let replacing = std::fs::symlink_metadata(&destination).is_ok();
        if replacing {
            create_dir_all(&aside)?;
            std::fs::rename(&destination, &set_aside)
                .map_err(|error| io_error(&destination, error))?;
        }
        if let Err(error) = std::fs::rename(unpacked, &destination) {
            if replacing && std::fs::rename(&set_aside, &destination).is_err() {
                return Err(EngineInstallError::Stranded {
                    kept: set_aside.to_string_lossy().into_owned(),
                    reason: format!("{}: {error}", destination.display()),
                });
            }
            return Err(io_error(&destination, error));
        }
        Ok(destination)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn an_install_dropped_before_it_commits_commits_nothing() {
        let landing = Landing::default();
        let dropped = Dropped::watching(&landing);
        let commit = dropped.commit();
        drop(dropped);
        let ran = AtomicBool::new(false);
        let result = commit.run("uv", || {
            ran.store(true, Ordering::SeqCst);
            Ok(PathBuf::from("/in/place"))
        });
        assert_eq!(
            result,
            Err(EngineInstallError::Stopped {
                engine: "uv".to_owned()
            })
        );
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(landing.landed(), None);
    }

    #[test]
    fn dropping_an_install_while_it_commits_waits_and_then_says_where_it_landed() {
        let landing = Landing::default();
        let dropped = Dropped::watching(&landing);
        let commit = dropped.commit();
        let started = Arc::new(Barrier::new(2));
        let committing = std::thread::spawn({
            let started = Arc::clone(&started);
            move || {
                commit.run("uv", || {
                    started.wait();
                    std::thread::sleep(Duration::from_millis(200));
                    Ok(PathBuf::from("/in/place"))
                })
            }
        });
        started.wait();
        drop(dropped);
        assert_eq!(landing.landed(), Some(PathBuf::from("/in/place")));
        assert_eq!(committing.join().unwrap(), Ok(PathBuf::from("/in/place")));
    }

    #[test]
    fn a_commit_that_fails_lands_nothing() {
        let landing = Landing::default();
        let dropped = Dropped::watching(&landing);
        let failed = dropped
            .commit()
            .run("uv", || Err(io_error(Path::new("/x"), "no room")));
        assert!(failed.is_err());
        assert_eq!(landing.landed(), None);
    }

    #[test]
    fn an_install_nothing_drops_always_commits() {
        assert_eq!(
            Commit::undroppable().run("llama.cpp", || Ok(PathBuf::from("/b"))),
            Ok(PathBuf::from("/b"))
        );
    }
}
