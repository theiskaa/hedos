//! Installing the pinned llama.cpp build, and removing it, when a person asks:
//! `hedos engines install` and the shelf's install action, after saying what
//! will be fetched and getting a yes. Unpacking the build this binary carries,
//! the first time a model runs, goes through the same steps and fetches
//! nothing.
//!
//! A build is the release's whole `llama-b<N>/` directory: `llama-server` and
//! the libraries it loads from beside itself, symlinks included. It is
//! streamed to disk (a CUDA build and its runtime are most of a gigabyte),
//! checked against the pinned sha256, unpacked into a staging directory that
//! refuses any entry reaching outside it, run once with `--version` to see it
//! is the build pinned, and only then moved into place.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use fs2::FileExt;
use kernel::engines::llama_cpp::{
    Asset, INSTALLED_FILE, InstallSource, Installed, InstalledFile, LlamaVersion, PINNED, Release,
    Variant, install_dir_name, loader_failure, parse_build_tag, parse_llama_version,
};
use kernel::machine::Os;
use sha2::{Digest, Sha256};

use tokio::io::AsyncWriteExt;

use super::bundled::{Bundled, bundled};
use super::llama_cpp::{LlamaServer, Source, hedos_root};

/// How long connecting to the release host may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the download may go without a byte before it is given up on.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// The lock file, in hedos's llama.cpp root, held while one install or
/// removal runs.
const LOCK_FILE: &str = ".lock";
/// How long taking the install lock is retried before an install is taken to
/// be running.
const LOCK_PATIENCE: Duration = Duration::from_secs(1);
/// How often it is retried meanwhile.
const LOCK_RETRY: Duration = Duration::from_millis(20);
/// How long unpacking the carried build waits for another process holding
/// the lock before saying llama.cpp is being installed.
const UNPACK_WAIT: Duration = Duration::from_secs(60);
/// The prefix of a staging directory an install unpacks into.
const STAGING_PREFIX: &str = ".staging-";
/// How long the unpacked `llama-server` may take to say its version.
const SMOKE_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of what a failing `llama-server` wrote an error carries.
const SMOKE_SAID: usize = 600;
/// The most of what `llama-server --version` writes that is read.
const SMOKE_READ: u64 = 64 * 1024;

/// Why llama.cpp could not be installed or removed.
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
    /// Another install or removal holds the lock.
    #[error("another llama.cpp install is running; try again when it finishes")]
    Busy,
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
    /// The unpacked `llama-server` is not the build pinned.
    #[error("the unpacked llama-server is not llama.cpp {expected}: {found}")]
    Smoke {
        /// The build expected.
        expected: String,
        /// What it reported, or that it reported nothing.
        found: String,
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
    /// The unpacked `llama-server` is being asked its version.
    Checking,
}

/// Install the pinned llama.cpp build of `variant` for this machine under
/// `data`, reporting through `progress`.
pub async fn install(
    variant: Variant,
    data: &Path,
    progress: impl Fn(InstallProgress),
) -> Result<LlamaServer, EngineInstallError> {
    let arch = std::env::consts::ARCH;
    let asset =
        PINNED
            .asset(Os::current(), arch, variant)
            .ok_or_else(|| EngineInstallError::NoBuild {
                version: PINNED.version.to_owned(),
                variant: variant.as_str().to_owned(),
                os: Os::current().as_str().to_owned(),
                arch: arch.to_owned(),
            })?;
    super::runs_here(asset).map_err(|refusal| EngineInstallError::WontRun {
        reason: refusal.to_string(),
    })?;
    install_from(
        &PINNED,
        asset,
        &PINNED.url(asset.publisher, ""),
        data,
        progress,
    )
    .await
}

/// Install `asset` of `release`, fetched from `base` (each archive at
/// `{base}{file}`), under `data`.
pub(crate) async fn install_from(
    release: &Release,
    asset: &Asset,
    base: &str,
    data: &Path,
    progress: impl Fn(InstallProgress),
) -> Result<LlamaServer, EngineInstallError> {
    let root = hedos_root(data);
    create_dir_all(&root)?;
    // Taking the lock may wait a moment (`lock`), and putting right what an
    // interrupted install left can delete most of a gigabyte: neither may
    // stall this runtime's other tasks.
    let preparing = root.clone();
    let (lock, staging) = tokio::task::spawn_blocking(move || {
        let lock = lock(&preparing)?;
        recover(&preparing)?;
        let staging = fresh_staging(&preparing)?;
        Ok::<_, EngineInstallError>((lock, staging))
    })
    .await
    .map_err(|error| io_error(&root, error))??;
    let lock = Arc::new(lock);
    let result = install_into(release, asset, base, &root, &staging, &lock, &progress).await;
    if !matches!(result, Err(EngineInstallError::Stranded { .. })) {
        let leftover = staging.clone();
        let held = Arc::clone(&lock);
        let _ = tokio::task::spawn_blocking(move || {
            let _held = held;
            std::fs::remove_dir_all(leftover)
        })
        .await;
    }
    result
}

async fn install_into(
    release: &Release,
    asset: &Asset,
    base: &str,
    root: &Path,
    staging: &Path,
    lock: &Arc<File>,
    progress: &impl Fn(InstallProgress),
) -> Result<LlamaServer, EngineInstallError> {
    let archives: Vec<(&str, &str, u64)> = std::iter::once((asset.file, asset.sha256, asset.bytes))
        .chain(
            asset
                .cudart
                .map(|runtime| (runtime.file, runtime.sha256, runtime.bytes)),
        )
        .collect();
    let total = asset.download_bytes();
    let mut received = 0;
    for (file, sha256, bytes) in &archives {
        let url = format!("{base}{file}");
        received = download(
            &url,
            &staging.join(file),
            sha256,
            *bytes,
            received,
            total,
            progress,
        )
        .await?;
    }

    progress(InstallProgress::Unpacking);
    let unpacked = staging.join("build");
    let to_unpack: Vec<(PathBuf, String)> = archives
        .iter()
        .map(|(file, _, _)| (staging.join(file), (*file).to_owned()))
        .collect();
    let built = Built {
        version: release.version.to_owned(),
        build: release.build.to_owned(),
        build_number: release.build_number(),
        variant: asset.variant,
        sha256: asset.sha256.to_owned(),
        source: InstallSource::Requested,
    };
    // The lock goes with each blocking step: an install dropped meanwhile
    // cannot stop a blocking task that has started, so none may begin until
    // it ends.
    let held = Arc::clone(lock);
    let target = unpacked.clone();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        to_unpack.iter().try_for_each(|(archive, name)| {
            let file = File::open(archive).map_err(|error| io_error(archive, error))?;
            unpack(GzDecoder::new(file), name, &target)
        })
    })
    .await
    .map_err(|error| io_error(&unpacked, error))??;

    progress(InstallProgress::Checking);
    let held = Arc::clone(lock);
    let (root, staging) = (root.to_path_buf(), staging.to_path_buf());
    let checked = unpacked.clone();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        finish(&built, &root, &staging, &checked)
    })
    .await
    .map_err(|error| io_error(&unpacked, error))?
}

/// What the build being put in place is, for its record.
struct Built {
    version: String,
    build: String,
    build_number: Option<u64>,
    variant: Variant,
    sha256: String,
    source: InstallSource,
}

/// Put the build unpacked at `unpacked` in place under `root`, once its
/// `llama-server` says it is the build expected: write its record, move a
/// build of the same name aside (put back if the new one cannot take its
/// place), and then remove the other builds of the same source. A build from
/// the other source is never touched: the one hedos carries and one installed
/// on request live side by side.
fn finish(
    built: &Built,
    root: &Path,
    staging: &Path,
    unpacked: &Path,
) -> Result<LlamaServer, EngineInstallError> {
    match smoke(&unpacked.join("llama-server"), &staging.join("smoke.log")) {
        Ok(version) if Some(version.build) == built.build_number => {}
        Ok(version) => {
            return Err(EngineInstallError::Smoke {
                expected: built.build.clone(),
                found: format!("it is {}", version.label()),
            });
        }
        Err(found) => {
            return Err(EngineInstallError::Smoke {
                expected: built.build.clone(),
                found,
            });
        }
    }

    let record = Installed {
        version: built.version.clone(),
        build: built.build.clone(),
        variant: built.variant,
        sha256: built.sha256.clone(),
        installed_at: kernel::time::now_millis(),
        source: built.source,
        files: inventory(unpacked)?,
    };
    let encoded = serde_json::to_vec_pretty(&record).map_err(|error| io_error(unpacked, error))?;
    let record_path = unpacked.join(INSTALLED_FILE);
    std::fs::write(&record_path, encoded).map_err(|error| io_error(&record_path, error))?;

    let name = install_dir_name(&built.build, built.variant, built.source);
    let destination = root.join(&name);
    let aside = staging.join("replaced");
    let set_aside = aside.join(&name);
    let replacing = std::fs::symlink_metadata(&destination).is_ok();
    if replacing {
        create_dir_all(&aside)?;
        std::fs::rename(&destination, &set_aside).map_err(|error| io_error(&destination, error))?;
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
    for other in installs(root)? {
        let Some((source, build)) = origin_of(&other) else {
            continue;
        };
        // A newer build of the bundle belongs to a newer hedos, which may be
        // running beside this one during an upgrade: removing it would have the
        // two unpack over each other at every launch.
        let superseded = source == InstallSource::Requested
            || parse_build_tag(&built.build).is_some_and(|this| build <= this);
        if other != destination && source == built.source && superseded {
            let _ = remove_entry(&other);
        }
    }
    Ok(LlamaServer {
        path: destination.join("llama-server"),
        from: match built.source {
            InstallSource::Requested => Source::Hedos,
            InstallSource::Bundled => Source::Bundled,
        },
    })
}

/// Every file directly in `dir`, with its size, or none for a link.
fn inventory(dir: &Path) -> Result<Vec<InstalledFile>, EngineInstallError> {
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

/// Where the build in `dir` came from and its build number: by its record,
/// else by the name an install gives its directory (`b11669-metal`,
/// `b11669-metal-bundled`), so a build whose record was lost is still cleaned
/// up with the others of its source. `None` for anything else, which is left
/// alone.
fn origin_of(dir: &Path) -> Option<(InstallSource, u64)> {
    let recorded = kernel::fs::read_regular(&dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Installed>(&bytes).ok())
        .and_then(|record| Some((record.source, parse_build_tag(&record.build)?)));
    if recorded.is_some() {
        return recorded;
    }
    let name = dir.file_name()?.to_str()?;
    let (named, source) = match name.strip_suffix("-bundled") {
        Some(rest) => (rest, InstallSource::Bundled),
        None => (name, InstallSource::Requested),
    };
    let (build, variant) = named.split_once('-')?;
    Variant::parse(variant)?;
    Some((source, parse_build_tag(build)?))
}

/// Unpack the llama.cpp build this binary carries under `data`, if it carries
/// one and it is not unpacked there already. Another process unpacking it is
/// waited for, not refused, and what it unpacked is then used.
pub(crate) fn unpack_bundled(data: &Path) -> Result<Option<LlamaServer>, EngineInstallError> {
    match bundled() {
        Some(bundle) => unpack_bundle(&bundle, &PINNED, data).map(Some),
        None => Ok(None),
    }
}

/// Unpack `bundle`, a build of `release`, under `data`, unless it is there.
pub(crate) fn unpack_bundle(
    bundle: &Bundled,
    release: &Release,
    data: &Path,
) -> Result<LlamaServer, EngineInstallError> {
    unpack_bundle_within(bundle, release, data, UNPACK_WAIT)
}

/// [`unpack_bundle`], waiting at most `wait` for another process holding the
/// lock (one unpacking the same build, or an install downloading).
fn unpack_bundle_within(
    bundle: &Bundled,
    release: &Release,
    data: &Path,
    wait: Duration,
) -> Result<LlamaServer, EngineInstallError> {
    let root = hedos_root(data);
    create_dir_all(&root)?;
    let name = install_dir_name(release.build, bundle.variant, InstallSource::Bundled);
    let unpacked = || LlamaServer {
        path: root.join(&name).join("llama-server"),
        from: Source::Bundled,
    };
    // An install on request can hold the lock for a whole download; a build
    // unpacked whole is used meanwhile instead of waited on. That install put
    // right what an interrupted one left when it took the lock.
    let lock = match try_lock(&root)? {
        Some(lock) => lock,
        None if unpacked_whole(&root.join(&name), bundle) => return Ok(unpacked()),
        None => lock_within(&root, wait)?,
    };
    // What an interrupted install left is put right first, whatever is found
    // unpacked: a build it set aside must not stay hidden behind this one.
    recover(&root)?;
    if unpacked_whole(&root.join(&name), bundle) {
        return Ok(unpacked());
    }
    let staging = fresh_staging(&root)?;
    let result = unpack_bundle_into(bundle, release, &root, &staging);
    if !matches!(result, Err(EngineInstallError::Stranded { .. })) {
        let _ = std::fs::remove_dir_all(&staging);
    }
    drop(lock);
    result
}

fn unpack_bundle_into(
    bundle: &Bundled,
    release: &Release,
    root: &Path,
    staging: &Path,
) -> Result<LlamaServer, EngineInstallError> {
    let found = hex::encode(Sha256::digest(bundle.archive));
    if found != bundle.sha256 {
        return Err(EngineInstallError::Checksum {
            url: "the llama.cpp build hedos carries".to_owned(),
            expected: bundle.sha256.to_owned(),
            found,
        });
    }
    let unpacked = staging.join("build");
    unpack(
        liblzma::read::XzDecoder::new(bundle.archive),
        "the llama.cpp build hedos carries",
        &unpacked,
    )?;
    let built = Built {
        version: release.version.to_owned(),
        build: release.build.to_owned(),
        build_number: release.build_number(),
        variant: bundle.variant,
        sha256: bundle.sha256.to_owned(),
        source: InstallSource::Bundled,
    };
    finish(&built, root, staging, &unpacked)
}

/// Whether `dir` holds `bundle` unpacked and whole: a directory of its own
/// (not a link), whose record names this bundle, and every file the record
/// lists still there, a file at its size and a link as a link, the server
/// among them.
fn unpacked_whole(dir: &Path, bundle: &Bundled) -> bool {
    if !std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir()) {
        return false;
    }
    let Some(record) = kernel::fs::read_regular(&dir.join(INSTALLED_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Installed>(&bytes).ok())
    else {
        return false;
    };
    let names_this = record.source == InstallSource::Bundled
        && record.sha256 == bundle.sha256
        && record.variant == bundle.variant;
    let has_server = record
        .files
        .iter()
        .any(|file| file.name == "llama-server" && file.bytes.is_some());
    names_this
        && has_server
        && record.files.iter().all(|file| {
            std::fs::symlink_metadata(dir.join(&file.name)).is_ok_and(|meta| match file.bytes {
                Some(bytes) => meta.is_file() && meta.len() == bytes,
                None => meta.file_type().is_symlink(),
            })
        })
        && kernel::fs::is_executable(&dir.join("llama-server"))
}

/// Remove every llama.cpp build hedos installed under `data`, returning the
/// directories removed. Nothing outside hedos's own root is touched.
pub fn remove(data: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
    let root = hedos_root(data);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let _lock = lock(&root)?;
    recover(&root)?;
    remove_installs(&root)
}

/// Finish what an install that did not end left in `root`, run under the
/// lock, so no install is running. A build one set aside goes back when
/// nothing of its name took its place, and is dropped when something did;
/// then the staging directory goes. One holding a build that cannot be put
/// back is kept and reported, so nothing goes on as if it were finished.
fn recover(root: &Path) -> Result<(), EngineInstallError> {
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

/// Remove the installs under `root`: every entry but the lock and staging
/// directories, a link removed as a link rather than followed.
fn remove_installs(root: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
    let mut removed = Vec::new();
    for path in installs(root)? {
        remove_entry(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

/// Remove `path`: a directory with what is in it, anything else (a link
/// included) as itself.
fn remove_entry(path: &Path) -> Result<(), EngineInstallError> {
    let is_dir = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    if is_dir {
        std::fs::remove_dir_all(path).map_err(|error| io_error(path, error))
    } else {
        std::fs::remove_file(path).map_err(|error| io_error(path, error))
    }
}

/// What is installed under `root`: every entry but hidden ones (the lock and
/// staging directories).
fn installs(root: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
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

/// Fetch `url` into `path`, refusing more than `bytes` and anything whose
/// sha256 is not `sha256`. Returns `received` plus what arrived.
async fn download(
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
fn unpack(archive: impl Read, name: &str, target: &Path) -> Result<(), EngineInstallError> {
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
            // llama.cpp links a library to its versioned file beside it; a
            // link anywhere else could lead out of the build.
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

fn create_dir_all(path: &Path) -> Result<(), EngineInstallError> {
    std::fs::create_dir_all(path).map_err(|error| io_error(path, error))
}

/// The install lock on `root`, taken without waiting on an install: a second
/// install while one runs is refused rather than queued behind a long
/// download. It is tried for up to `LOCK_PATIENCE`, since a process this one
/// is starting holds a copy of every open file, the lock among them, until it
/// execs, and that must not read as an install running.
fn lock(root: &Path) -> Result<File, EngineInstallError> {
    lock_within(root, LOCK_PATIENCE)
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
fn try_lock(root: &Path) -> Result<Option<File>, EngineInstallError> {
    let file = open_lock(&root.join(LOCK_FILE))?;
    Ok(file.try_lock_exclusive().is_ok().then_some(file))
}

/// The install lock on `root`, waited for up to `wait`: an unpack of the build
/// hedos carries that another process is doing takes seconds, and is waited
/// out; an install on request downloading can take minutes, and is refused
/// as [`EngineInstallError::Busy`] once `wait` runs out.
fn lock_within(root: &Path, wait: Duration) -> Result<File, EngineInstallError> {
    let file = open_lock(&root.join(LOCK_FILE))?;
    let started = Instant::now();
    while file.try_lock_exclusive().is_err() {
        if started.elapsed() >= wait {
            return Err(EngineInstallError::Busy);
        }
        std::thread::sleep(LOCK_RETRY);
    }
    Ok(file)
}

/// The version the `llama-server` at `server` reports, or what went wrong
/// asking: it could not start, it did not answer in time, or its exit status
/// and the end of what it wrote (a library it could not load, say). What it
/// writes goes to `log`, a file, not a pipe, so nothing is left waiting on a
/// pipe some process it started still holds.
fn smoke(server: &Path, log: &Path) -> Result<LlamaVersion, String> {
    let written = File::create(log).map_err(|error| format!("{}: {error}", log.display()))?;
    let mut child = crate::process::spawn_written(
        std::process::Command::new(server)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(written)),
    )
    .map_err(|error| format!("it could not start: {error}"))?;
    let deadline = Instant::now() + SMOKE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "it did not answer within {} s",
                    SMOKE_TIMEOUT.as_secs()
                ));
            }
        }
    };
    let mut said = Vec::new();
    if let Ok(file) = File::open(log) {
        let _ = file.take(SMOKE_READ).read_to_end(&mut said);
    }
    let said = String::from_utf8_lossy(&said);
    match parse_llama_version(&said) {
        Some(version) if status.success() => Ok(version),
        _ => {
            if let Some(reason) = loader_failure(&said) {
                return Err(reason);
            }
            let tail: String = said.trim().chars().rev().take(SMOKE_SAID).collect();
            let tail: String = tail.chars().rev().collect();
            Err(format!("it exited with {status} and said: {tail}"))
        }
    }
}

/// A new, empty staging directory under `root`, named for this process and
/// moment so no earlier one is reused.
fn fresh_staging(root: &Path) -> Result<PathBuf, EngineInstallError> {
    let staging = root.join(format!(
        "{STAGING_PREFIX}{}-{}",
        std::process::id(),
        kernel::time::now_millis()
    ));
    std::fs::create_dir(&staging).map_err(|error| io_error(&staging, error))?;
    Ok(staging)
}

fn io_error(path: &Path, error: impl std::fmt::Display) -> EngineInstallError {
    EngineInstallError::Io {
        path: path.to_string_lossy().into_owned(),
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::engines::llama_cpp::Publisher;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::engines::llama_cpp::{installed, locate};

    const RELEASE: Release = Release {
        version: "0.7.0",
        build: "b11669",
        source: PINNED.source,
        assets: &[],
    };

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hedos-engine-install-{name}-{}-{}",
                std::process::id(),
                kernel::time::now_millis()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One tar entry: a path written as is (so `..` and absolute paths can be
    /// tested), and a file's bytes, a directory, or a link's target.
    enum Item<'a> {
        File(&'a str, &'a [u8], u32),
        Dir(&'a str),
        Link(&'a str, &'a str),
    }

    fn archive(items: &[Item]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for item in items {
            let mut header = tar::Header::new_old();
            let (path, data, kind, mode): (&str, &[u8], tar::EntryType, u32) = match item {
                Item::File(path, data, mode) => (path, data, tar::EntryType::Regular, *mode),
                Item::Dir(path) => (path, &[], tar::EntryType::Directory, 0o755),
                Item::Link(path, target) => {
                    header.set_link_name(target).unwrap();
                    (path, &[], tar::EntryType::Symlink, 0o777)
                }
            };
            let name = &mut header.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_entry_type(kind);
            header.set_mode(mode);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn server_script(build: u64) -> Vec<u8> {
        format!("#!/bin/sh\necho 'version: 0.7.0 (build {build}, commit x)' >&2\n").into_bytes()
    }

    fn good_build() -> Vec<u8> {
        let script = server_script(11669);
        archive(&[
            Item::Dir("llama-b11669/"),
            Item::File("llama-b11669/llama-server", &script, 0o755),
            Item::File("llama-b11669/libggml.0.27.0.dylib", b"lib", 0o755),
            Item::Link("llama-b11669/libggml.0.dylib", "libggml.0.27.0.dylib"),
            Item::File("llama-b11669/LICENSE", b"MIT", 0o644),
        ])
    }

    /// An asset for `bytes` as published, its digest and size pinned.
    fn asset_of(bytes: &[u8], variant: Variant) -> Asset {
        Asset {
            os: Os::current(),
            arch: std::env::consts::ARCH,
            variant,
            file: "build.tar.gz",
            sha256: Box::leak(hex::encode(Sha256::digest(bytes)).into_boxed_str()),
            bytes: bytes.len() as u64,
            cudart: None,
            publisher: Publisher::LlamaCpp,
        }
    }

    /// Serve `files` by name over HTTP until dropped; returns the base URL.
    async fn serve(files: HashMap<&'static str, Vec<u8>>) -> (String, tokio::task::AbortHandle) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let files = Arc::new(files);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let files = Arc::clone(&files);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        let Ok(read) = stream.read(&mut buffer).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                    }
                    let head = String::from_utf8_lossy(&request);
                    let path = head.split_whitespace().nth(1).unwrap_or("/");
                    let response = match files.get(path.trim_start_matches('/')) {
                        Some(body) => {
                            let mut response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            response.extend_from_slice(body);
                            response
                        }
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
                    };
                    let _ = stream.write_all(&response).await;
                });
            }
        })
        .abort_handle();
        (base, accept)
    }

    async fn run(
        scratch: &Scratch,
        published: Vec<u8>,
        pinned: &Asset,
    ) -> Result<LlamaServer, EngineInstallError> {
        let (base, accept) = serve(HashMap::from([("build.tar.gz", published)])).await;
        let seen = Mutex::new(Vec::new());
        let result = install_from(&RELEASE, pinned, &base, &scratch.0, |event| {
            seen.lock().unwrap().push(event);
        })
        .await;
        accept.abort();
        result
    }

    fn nothing_but_the_lock(scratch: &Scratch) -> bool {
        std::fs::read_dir(hedos_root(&scratch.0))
            .unwrap()
            .flatten()
            .all(|entry| entry.file_name() == LOCK_FILE)
    }

    #[tokio::test]
    async fn a_pinned_build_installs_whole_and_is_the_one_located() {
        let scratch = Scratch::new("good");
        let published = good_build();
        let pinned = asset_of(&published, Variant::Metal);
        let server = run(&scratch, published, &pinned).await.expect("installed");

        let dir = hedos_root(&scratch.0).join("b11669-metal");
        assert_eq!(server.path, dir.join("llama-server"));
        assert_eq!(
            std::fs::read_link(dir.join("libggml.0.dylib")).unwrap(),
            Path::new("libggml.0.27.0.dylib")
        );
        let (found, record) = installed(&scratch.0, InstallSource::Requested).expect("an install");
        assert_eq!(found, dir);
        assert_eq!(
            (record.build.as_str(), record.variant),
            ("b11669", Variant::Metal)
        );
        assert_eq!(locate("", &scratch.0).unwrap().from, Source::Hedos);

        // Installing again replaces it, and leaves no staging behind.
        let published = good_build();
        let pinned = asset_of(&published, Variant::Cpu);
        run(&scratch, published, &pinned)
            .await
            .expect("installed again");
        assert!(!dir.exists());
        assert!(
            hedos_root(&scratch.0)
                .join("b11669-cpu/llama-server")
                .exists()
        );
        assert_eq!(
            std::fs::read_dir(hedos_root(&scratch.0)).unwrap().count(),
            2
        );
    }

    #[tokio::test]
    async fn an_archive_that_is_not_the_one_pinned_installs_nothing() {
        let scratch = Scratch::new("checksum");
        let pinned = asset_of(&good_build(), Variant::Metal);
        let tampered = archive(&[Item::File("llama-b11669/llama-server", b"other", 0o755)]);
        let pinned = Asset {
            bytes: 1 << 20,
            ..pinned
        };
        assert!(matches!(
            run(&scratch, tampered, &pinned).await,
            Err(EngineInstallError::Checksum { .. })
        ));
        assert!(nothing_but_the_lock(&scratch));
    }

    #[tokio::test]
    async fn an_archive_larger_than_pinned_is_cut_off() {
        let scratch = Scratch::new("oversize");
        let published = good_build();
        let pinned = Asset {
            bytes: 10,
            ..asset_of(&published, Variant::Metal)
        };
        assert!(matches!(
            run(&scratch, published, &pinned).await,
            Err(EngineInstallError::Download { .. })
        ));
        assert!(nothing_but_the_lock(&scratch));
    }

    #[tokio::test]
    async fn an_entry_that_leaves_the_build_refuses_the_archive() {
        let script = server_script(11669);
        for escaping in [
            archive(&[Item::File("llama-b11669/../../escaped", b"x", 0o644)]),
            archive(&[Item::File("/tmp/hedos-escaped-absolute", b"x", 0o644)]),
            archive(&[
                Item::File("llama-b11669/llama-server", &script, 0o755),
                Item::Link("llama-b11669/libx.dylib", "../../../etc/passwd"),
            ]),
        ] {
            let scratch = Scratch::new("escape");
            let pinned = asset_of(&escaping, Variant::Metal);
            assert!(matches!(
                run(&scratch, escaping, &pinned).await,
                Err(EngineInstallError::Archive { .. })
            ));
            assert!(nothing_but_the_lock(&scratch));
            assert!(!scratch.0.join("escaped").exists());
        }
        assert!(!Path::new("/tmp/hedos-escaped-absolute").exists());
    }

    #[tokio::test]
    async fn a_server_that_is_not_the_pinned_build_installs_nothing() {
        let scratch = Scratch::new("smoke");
        let script = server_script(10050);
        let published = archive(&[Item::File("llama-b11669/llama-server", &script, 0o755)]);
        let pinned = asset_of(&published, Variant::Metal);
        match run(&scratch, published, &pinned).await {
            Err(EngineInstallError::Smoke { found, .. }) => {
                assert!(found.contains("10050"), "{found}")
            }
            other => panic!("expected a smoke failure, got {other:?}"),
        }
        assert!(nothing_but_the_lock(&scratch));
    }

    #[tokio::test]
    async fn a_second_install_while_one_runs_is_refused() {
        let scratch = Scratch::new("busy");
        std::fs::create_dir_all(hedos_root(&scratch.0)).unwrap();
        let held = lock(&hedos_root(&scratch.0)).unwrap();
        let published = good_build();
        let pinned = asset_of(&published, Variant::Metal);
        assert_eq!(
            run(&scratch, published, &pinned).await,
            Err(EngineInstallError::Busy)
        );
        drop(held);
    }

    #[tokio::test]
    async fn removing_deletes_hedos_build_and_nothing_else() {
        let scratch = Scratch::new("remove");
        let published = good_build();
        let pinned = asset_of(&published, Variant::Metal);
        run(&scratch, published, &pinned).await.expect("installed");
        let outside = scratch.0.join("keep");
        std::fs::write(&outside, b"x").unwrap();
        std::os::unix::fs::symlink(&outside, hedos_root(&scratch.0).join("link")).unwrap();

        let removed = remove(&scratch.0).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(installed(&scratch.0, InstallSource::Requested).is_none());
        assert!(outside.exists(), "a link is removed, not followed");
        assert!(remove(&Scratch::new("empty").0).unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_install_cut_short_is_finished_by_the_next_one() {
        let scratch = Scratch::new("recover");
        let root = hedos_root(&scratch.0);
        // What an install stopped between moving the old build aside and
        // putting the new one in place leaves: the old build in staging.
        let aside = root.join(".staging-1-1/replaced/b11669-metal");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("llama-server"), b"old").unwrap();
        let abandoned = root.join(".staging-2-2");
        std::fs::create_dir_all(abandoned.join("build")).unwrap();

        remove(&scratch.0).unwrap();
        assert!(nothing_but_the_lock(&scratch), "put back, then removed");

        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("llama-server"), b"old").unwrap();
        let _lock = lock(&root).unwrap();
        recover(&root).unwrap();
        assert!(root.join("b11669-metal/llama-server").exists());
        assert!(!root.join(".staging-1-1").exists());
    }

    #[tokio::test]
    async fn a_lock_file_that_is_a_link_touches_nothing_it_points_to() {
        let scratch = Scratch::new("lock-link");
        let outside = scratch.0.join("precious");
        std::fs::write(&outside, b"keep").unwrap();
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(LOCK_FILE)).unwrap();
        assert!(matches!(lock(&root), Err(EngineInstallError::Io { .. })));
        assert!(remove(&scratch.0).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_says_why() {
        let scratch = Scratch::new("smoke-why");
        let failing =
            b"#!/bin/sh\necho 'dyld: Library not loaded: libggml.0.dylib' >&2\nexit 134\n";
        let published = archive(&[Item::File("llama-b11669/llama-server", failing, 0o755)]);
        let pinned = asset_of(&published, Variant::Metal);
        match run(&scratch, published, &pinned).await {
            Err(EngineInstallError::Smoke { found, .. }) => {
                assert!(found.contains("Library not loaded"), "{found}");
                assert!(found.contains("134"), "{found}");
            }
            other => panic!("expected a smoke failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn recovery_puts_back_only_what_nothing_replaced() {
        let scratch = Scratch::new("recover-by-name");
        let root = hedos_root(&scratch.0);
        let aside = root.join(".staging-3-3/replaced");
        for name in ["b11669-metal", "b11669-cpu"] {
            std::fs::create_dir_all(aside.join(name)).unwrap();
            std::fs::write(aside.join(name).join("llama-server"), b"old").unwrap();
        }
        // The cpu build was replaced by a new one of its name; the metal one
        // was not, so only it goes back, and the new cpu build is untouched.
        std::fs::create_dir_all(root.join("b11669-cpu")).unwrap();
        std::fs::write(root.join("b11669-cpu/llama-server"), b"new").unwrap();
        let _lock = lock(&root).unwrap();
        recover(&root).unwrap();
        assert_eq!(
            std::fs::read(root.join("b11669-metal/llama-server")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(root.join("b11669-cpu/llama-server")).unwrap(),
            b"new"
        );
        assert!(!root.join(".staging-3-3").exists());
    }

    /// `release_gz`, a release archive as the build script receives it, as the
    /// binary would carry it: cut down, compressed with xz, and its digest.
    fn bundle_of(release_gz: &[u8], variant: Variant) -> Bundled {
        let tar = kernel::engines::bundle::server_files(release_gz).unwrap();
        let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 1);
        encoder.write_all(&tar).unwrap();
        let archive: &'static [u8] = Box::leak(encoder.finish().unwrap().into_boxed_slice());
        Bundled {
            archive,
            sha256: Box::leak(hex::encode(Sha256::digest(archive)).into_boxed_str()),
            variant,
        }
    }

    fn record_in(dir: &Path) -> Installed {
        serde_json::from_slice(&std::fs::read(dir.join(INSTALLED_FILE)).unwrap()).unwrap()
    }

    #[test]
    fn the_carried_build_unpacks_once_into_a_directory_of_its_own() {
        let scratch = Scratch::new("bundle");
        let bundle = bundle_of(&good_build(), Variant::Metal);
        let server = unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked");
        let dir = hedos_root(&scratch.0).join("b11669-metal-bundled");
        assert_eq!(server.path, dir.join("llama-server"));
        assert_eq!(record_in(&dir).source, InstallSource::Bundled);
        assert_eq!(
            std::fs::read_link(dir.join("libggml.0.dylib")).unwrap(),
            Path::new("libggml.0.27.0.dylib")
        );

        // Asked again, it is already there: nothing is unpacked a second time.
        let marker = dir.join("marker");
        std::fs::write(&marker, b"x").unwrap();
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("there");
        assert!(marker.exists());
        assert!(nothing_hidden_but_the_lock(&scratch));
    }

    #[test]
    fn a_carried_build_that_is_not_what_its_digest_says_unpacks_nothing() {
        let scratch = Scratch::new("bundle-digest");
        let bundle = Bundled {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..bundle_of(&good_build(), Variant::Metal)
        };
        assert!(matches!(
            unpack_bundle(&bundle, &RELEASE, &scratch.0),
            Err(EngineInstallError::Checksum { .. })
        ));
        assert!(nothing_but_the_lock(&scratch));
    }

    #[tokio::test]
    async fn the_carried_build_and_one_installed_on_request_never_remove_each_other() {
        let scratch = Scratch::new("bundle-both");
        let bundle = bundle_of(&good_build(), Variant::Cpu);
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked");
        let published = good_build();
        let pinned = asset_of(&published, Variant::Metal);
        run(&scratch, published, &pinned).await.expect("installed");
        let root = hedos_root(&scratch.0);
        assert!(root.join("b11669-cpu-bundled/llama-server").exists());
        assert!(root.join("b11669-metal/llama-server").exists());

        // Another carried build, of this release or a newer one, replaces the
        // carried one there, and only it.
        unpack_bundle(
            &bundle_of(&good_build(), Variant::Metal),
            &RELEASE,
            &scratch.0,
        )
        .expect("unpacked");
        assert!(root.join("b11669-metal-bundled/llama-server").exists());
        assert!(!root.join("b11669-cpu-bundled").exists(), "replaced");
        assert!(root.join("b11669-metal/llama-server").exists());
    }

    #[test]
    fn a_carried_build_unpacked_whole_is_used_while_an_install_holds_the_lock() {
        let scratch = Scratch::new("bundle-busy");
        let root = hedos_root(&scratch.0);
        let bundle = bundle_of(&good_build(), Variant::Metal);
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked");
        // An install on request downloading, holding the lock.
        let held = lock(&root).unwrap();
        let data = scratch.0.clone();
        let using = std::thread::spawn(move || unpack_bundle(&bundle, &RELEASE, &data));
        std::thread::sleep(Duration::from_millis(300));
        let finished = using.is_finished();
        drop(held);
        let server = using.join().unwrap().expect("the unpacked build");
        assert!(finished, "it does not wait for the install");
        assert_eq!(server.path, root.join("b11669-metal-bundled/llama-server"));
    }

    #[test]
    fn a_carried_build_not_unpacked_waits_for_an_install_only_so_long() {
        let scratch = Scratch::new("bundle-wait-bound");
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(&root).unwrap();
        // An install on request downloading, holding the lock throughout.
        let held = lock(&root).unwrap();
        let started = Instant::now();
        let refused = unpack_bundle_within(
            &bundle_of(&good_build(), Variant::Metal),
            &RELEASE,
            &scratch.0,
            Duration::from_millis(200),
        );
        assert!(
            matches!(refused, Err(EngineInstallError::Busy)),
            "{refused:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(held);
    }

    #[test]
    fn a_second_unpack_waits_for_the_first_and_uses_what_it_unpacked() {
        let scratch = Scratch::new("bundle-wait");
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(&root).unwrap();
        let held = lock(&root).unwrap();
        let data = scratch.0.clone();
        let waiting = std::thread::spawn(move || {
            unpack_bundle(&bundle_of(&good_build(), Variant::Metal), &RELEASE, &data)
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(!waiting.is_finished(), "it waits for the lock");
        drop(held);
        assert!(waiting.join().unwrap().is_ok());
    }

    fn nothing_hidden_but_the_lock(scratch: &Scratch) -> bool {
        std::fs::read_dir(hedos_root(&scratch.0))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .all(|entry| entry.file_name() == LOCK_FILE)
    }

    #[test]
    fn a_carried_build_cut_short_or_missing_a_library_is_unpacked_again() {
        let scratch = Scratch::new("bundle-damaged");
        let bundle = bundle_of(&good_build(), Variant::Metal);
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked");
        let dir = hedos_root(&scratch.0).join("b11669-metal-bundled");
        let library = dir.join("libggml.0.27.0.dylib");

        std::fs::write(&library, b"").unwrap();
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked again");
        assert_eq!(std::fs::read(&library).unwrap(), b"lib");

        std::fs::remove_file(&library).unwrap();
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked again");
        assert!(library.exists());
    }

    #[test]
    fn a_carried_build_reached_through_a_link_is_not_taken_for_unpacked() {
        let scratch = Scratch::new("bundle-link");
        let elsewhere = Scratch::new("bundle-link-elsewhere");
        let bundle = bundle_of(&good_build(), Variant::Metal);
        unpack_bundle(&bundle, &RELEASE, &elsewhere.0).expect("unpacked elsewhere");
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(&root).unwrap();
        let name = root.join("b11669-metal-bundled");
        std::os::unix::fs::symlink(hedos_root(&elsewhere.0).join("b11669-metal-bundled"), &name)
            .unwrap();
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked here");
        assert!(std::fs::symlink_metadata(&name).unwrap().is_dir());
    }

    #[test]
    fn a_build_whose_record_was_lost_is_still_cleaned_up_with_its_source() {
        let scratch = Scratch::new("bundle-orphan");
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(root.join("b11000-metal-bundled")).unwrap();
        std::fs::create_dir_all(root.join("b11000-cpu")).unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        unpack_bundle(
            &bundle_of(&good_build(), Variant::Metal),
            &RELEASE,
            &scratch.0,
        )
        .expect("unpacked");
        assert!(
            !root.join("b11000-metal-bundled").exists(),
            "an older carried build"
        );
        assert!(
            root.join("b11000-cpu").exists(),
            "one installed on request stays"
        );
        assert!(root.join("notes").exists(), "anything else is left alone");
    }

    #[test]
    fn a_newer_carried_build_another_hedos_unpacked_is_left_for_it() {
        let scratch = Scratch::new("bundle-newer");
        let root = hedos_root(&scratch.0);
        std::fs::create_dir_all(root.join("b99999-metal-bundled")).unwrap();
        // The record, not the name, says which build a directory holds: a
        // newer one named as if older stays, an older one named as nothing
        // goes.
        let recorded = root.join("b100-cpu-bundled");
        std::fs::create_dir_all(&recorded).unwrap();
        let record = Installed {
            version: "9.9.9".to_owned(),
            build: "b99998".to_owned(),
            variant: Variant::Cpu,
            sha256: "0".repeat(64),
            installed_at: 0,
            source: InstallSource::Bundled,
            files: Vec::new(),
        };
        std::fs::write(
            recorded.join(INSTALLED_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let unnamed = root.join("carried");
        std::fs::create_dir_all(&unnamed).unwrap();
        let mut older = record;
        older.build = "b100".to_owned();
        std::fs::write(
            unnamed.join(INSTALLED_FILE),
            serde_json::to_vec(&older).unwrap(),
        )
        .unwrap();
        unpack_bundle(
            &bundle_of(&good_build(), Variant::Metal),
            &RELEASE,
            &scratch.0,
        )
        .expect("unpacked");
        assert!(root.join("b99999-metal-bundled").exists());
        assert!(recorded.exists());
        assert!(!unnamed.exists());
        assert!(root.join("b11669-metal-bundled/llama-server").exists());
    }

    #[test]
    fn a_requested_build_an_interrupted_install_set_aside_comes_back_first() {
        let scratch = Scratch::new("bundle-recover");
        let bundle = bundle_of(&good_build(), Variant::Metal);
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("unpacked");
        let root = hedos_root(&scratch.0);
        let aside = root.join(".staging-9-9/replaced/b11669-cpu");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("llama-server"), b"requested").unwrap();
        unpack_bundle(&bundle, &RELEASE, &scratch.0).expect("there");
        assert!(root.join("b11669-cpu/llama-server").exists());
        assert!(!root.join(".staging-9-9").exists());
    }

    /// How a pin's archive for this machine is checked before it is
    /// published: installed through the download path, it is the size and
    /// digest pinned and its `llama-server` starts here as the pinned build.
    #[tokio::test]
    #[ignore = "needs HEDOS_PINNED_ARCHIVE, a pinned llama.cpp archive for this machine"]
    async fn a_pinned_archive_installs_and_reports_the_pinned_version() {
        let path = std::env::var("HEDOS_PINNED_ARCHIVE").expect("HEDOS_PINNED_ARCHIVE");
        let published = std::fs::read(&path).expect("the archive");
        let digest = hex::encode(Sha256::digest(&published));
        let asset = PINNED
            .assets
            .iter()
            .find(|asset| {
                asset.sha256 == digest
                    && asset.os == Os::current()
                    && asset.arch == std::env::consts::ARCH
                    && asset.cudart.is_none()
            })
            .expect("an archive the pin has for this machine, without a runtime archive beside it");
        let scratch = Scratch::new("pinned-real");
        let (base, accept) = serve(HashMap::from([(asset.file, published)])).await;
        let server = install_from(&PINNED, asset, &base, &scratch.0, |_| {})
            .await
            .expect("installed");
        accept.abort();
        let version = crate::engines::llama_cpp::version(&server.path).expect("a version");
        assert_eq!(Some(version.build), PINNED.build_number());
    }

    #[cfg(hedos_bundled_llama)]
    #[test]
    fn the_carried_build_unpacks_and_reports_the_pinned_version() {
        let scratch = Scratch::new("carried-real");
        let server = unpack_bundled(&scratch.0)
            .expect("unpacked")
            .expect("a bundle");
        assert_eq!(server.from, Source::Bundled);
        let found = locate("", &scratch.0).expect("located");
        assert_eq!(
            (found.path, found.from),
            (server.path.clone(), Source::Bundled)
        );
        let version = crate::engines::llama_cpp::version(&server.path).expect("a version");
        assert_eq!(Some(version.build), PINNED.build_number());
    }
}
