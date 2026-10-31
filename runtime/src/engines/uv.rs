//! Which `uv` builds the Python runtimes' environments, and installing the
//! pinned one when a person asks: `hedos engines install uv`, or the question
//! after a pull. One place decides for every surface: the uv hedos installed,
//! else one where uv's installer or Homebrew puts it, else the one on PATH.
//!
//! A version hedos installs lives in its own directory, `uv` and `uvx` with
//! the record of what was put there, and is checked to say it is the version
//! pinned before it is moved into place.

use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flate2::read::GzDecoder;
use kernel::engines::INSTALLED_FILE;
use kernel::engines::uv::{Asset, Installed, PINNED, Release, parse_version};
use kernel::machine::Os;
use kernel::records::RuntimeId;

use super::fetch::{
    Commit, Dropped, EngineInstallError, InstallProgress, Landing, Site, ask_version,
    create_dir_all, download, fresh_staging, inventory, io_error, lock, put_in_place, recover,
    remove_installs, try_lock,
};
use crate::boot::{self, HedosDirs};
use crate::environment::EnvironmentManager;
use crate::machine::{Stream, run_bounded_reading};
use crate::sidecar::RuntimeBundle;

/// The engine, as errors name it.
const ENGINE: &str = "uv";
/// The program that builds the environments.
const PROGRAM: &str = "uv";
/// How long `uv --version` may take when asked for the engines list.
const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// Where a `uv` was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A version hedos installed because someone asked for it.
    Hedos,
    /// Where uv's installer or Homebrew puts it, or the PATH.
    Path,
}

impl Source {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Hedos => "hedos",
            Source::Path => "path",
        }
    }
}

/// A `uv` hedos can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uv {
    /// The executable.
    pub path: PathBuf,
    /// Where it was found.
    pub from: Source,
}

/// What an install did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It fetched the pinned version and put it in place.
    Installed,
    /// The pinned version was already there, whole, so nothing was fetched.
    Present,
}

/// Where hedos keeps the uv versions it installs, under the data dir.
pub fn hedos_root(data: &Path) -> PathBuf {
    data.join("engines").join("uv")
}

/// The `uv` to run with this process's data dir: the first of a version
/// hedos installed (the pinned one, else the one installed last), then
/// [`elsewhere`].
pub fn located() -> Option<Uv> {
    locate(&HedosDirs::detect().data, elsewhere)
}

/// The `uv` to run, the first of: a version hedos installed under `data`
/// (the pinned one, else the one installed last); what `elsewhere` finds.
/// hedos's own comes first, so every machine with it builds environments
/// with the uv this hedos was tested with.
fn locate(data: &Path, elsewhere: impl FnOnce() -> Option<PathBuf>) -> Option<Uv> {
    if let Some((dir, _)) = installed(data) {
        return Some(Uv {
            path: dir.join(PROGRAM),
            from: Source::Hedos,
        });
    }
    elsewhere().map(|path| Uv {
        path,
        from: Source::Path,
    })
}

/// A `uv` not hedos's: where uv's installer (`~/.local/bin`) or Homebrew
/// puts it, then the PATH. The fixed places come first because a process
/// started from a desktop or a service often has none of them on its PATH.
fn elsewhere() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        format!("{home}/.local/bin/{PROGRAM}"),
        format!("/opt/homebrew/bin/{PROGRAM}"),
        format!("/usr/local/bin/{PROGRAM}"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|candidate| candidate.is_absolute() && kernel::fs::is_executable(candidate))
    .or_else(|| kernel::fs::find_absolute_on_path(PROGRAM))
}

/// The version hedos installed under `data` and what its record says: the
/// pinned version when it is there, else the one installed last. Only a
/// directory of its own counts (not a link out of the root), named for the
/// version its record says, built for this machine, with every file the
/// record lists still there at its size and `uv` executable.
pub fn installed(data: &Path) -> Option<(PathBuf, Installed)> {
    let (os, arch) = (Os::current().as_str(), std::env::consts::ARCH);
    let entries = std::fs::read_dir(hedos_root(data)).ok()?;
    let mut found: Vec<(PathBuf, Installed)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let dir = entry.path();
            let bytes = kernel::fs::read_regular(&dir.join(INSTALLED_FILE)).ok()?;
            let record = serde_json::from_slice::<Installed>(&bytes).ok()?;
            let named = entry.file_name().to_str() == Some(record.version.as_str());
            let here = record.os == os && record.arch == arch;
            (named && here && whole(&dir, &record) && kernel::fs::is_executable(&dir.join(PROGRAM)))
                .then_some((dir, record))
        })
        .collect();
    found.sort_by_key(|(_, record)| (record.version == PINNED.version, record.installed_at));
    found.pop()
}

/// Whether every file `record` lists is in `dir` as a plain file of its size.
fn whole(dir: &Path, record: &Installed) -> bool {
    !record.files.is_empty()
        && record.files.iter().all(|file| {
            std::fs::symlink_metadata(dir.join(&file.name))
                .is_ok_and(|meta| meta.is_file() && Some(meta.len()) == file.bytes)
        })
}

/// The pinned uv for this machine when hedos installed it under `data`, it
/// is whole, and it still says it is the version pinned. What an interrupted
/// install left is put right first, unless an install holds the lock now, so
/// asking leaves nothing behind for long.
pub fn present(data: &Path) -> Option<Uv> {
    let asset = PINNED.asset(Os::current(), std::env::consts::ARCH)?;
    let root = hedos_root(data);
    if root.is_dir()
        && let Ok(Some(_lock)) = try_lock(&root)
    {
        let _ = recover(&root);
    }
    pinned_in(data, PINNED.version, asset.sha256)
}

/// The install of `version` from the archive `sha256` under `data`, when it
/// is whole and its `uv` answers with that version: a file damaged without
/// changing its size is caught by asking, which its record cannot do.
fn pinned_in(data: &Path, version: &str, sha256: &str) -> Option<Uv> {
    installed(data)
        .filter(|(dir, record)| {
            record.version == version
                && record.sha256 == sha256
                && self::version(&dir.join(PROGRAM)).as_deref() == Some(version)
        })
        .map(|(dir, _)| Uv {
            path: dir.join(PROGRAM),
            from: Source::Hedos,
        })
}

/// The version the `uv` at `path` reports.
pub fn version(path: &Path) -> Option<String> {
    run_bounded_reading(path, &["--version"], VERSION_TIMEOUT, Stream::Stdout)
        .as_deref()
        .and_then(parse_version)
        .map(str::to_owned)
}

/// Install the pinned uv for this machine under `data`, reporting through
/// `progress`. When it is already there, whole, nothing is fetched.
/// `landing` says where it put uv, for a caller that drops it (Ctrl-C) as it
/// was finishing.
pub async fn install(
    data: &Path,
    landing: &Landing,
    progress: impl Fn(InstallProgress),
) -> Result<(Uv, Outcome), EngineInstallError> {
    let arch = std::env::consts::ARCH;
    let asset =
        PINNED
            .asset(Os::current(), arch)
            .ok_or_else(|| EngineInstallError::Unsupported {
                engine: ENGINE.to_owned(),
                version: PINNED.version.to_owned(),
                os: Os::current().as_str().to_owned(),
                arch: arch.to_owned(),
            })?;
    install_from(&PINNED, asset, &PINNED.url(""), data, landing, progress).await
}

/// Install `asset` of `release`, fetched from `base` (the archive at
/// `{base}{file}`), under `data`.
async fn install_from(
    release: &Release,
    asset: &Asset,
    base: &str,
    data: &Path,
    landing: &Landing,
    progress: impl Fn(InstallProgress),
) -> Result<(Uv, Outcome), EngineInstallError> {
    let root = hedos_root(data);
    create_dir_all(&root)?;
    // Whether it is already there is asked under the lock, after what an
    // interrupted install left is put right: an install that finished while
    // this one waited is not fetched again, and leftovers never outlive it.
    let preparing = root.clone();
    let (data, version, sha256) = (data.to_path_buf(), release.version, asset.sha256);
    let (lock, staging) = tokio::task::spawn_blocking(move || {
        let lock = lock(&preparing, ENGINE)?;
        recover(&preparing)?;
        if let Some(present) = pinned_in(&data, version, sha256) {
            return Ok((lock, Err(present)));
        }
        let staging = fresh_staging(&preparing)?;
        Ok::<_, EngineInstallError>((lock, Ok(staging)))
    })
    .await
    .map_err(|error| io_error(&root, error))??;
    let staging = match staging {
        Ok(staging) => staging,
        Err(present) => return Ok((present, Outcome::Present)),
    };
    let lock = Arc::new(lock);
    let site = Site {
        root: &root,
        staging: &staging,
        lock: &lock,
        landing,
    };
    let result = install_into(release, asset, base, &site, &progress).await;
    if !matches!(result, Err(EngineInstallError::Stranded { .. })) {
        let leftover = staging.clone();
        let held = Arc::clone(&lock);
        let _ = tokio::task::spawn_blocking(move || {
            let _held = held;
            std::fs::remove_dir_all(leftover)
        })
        .await;
    }
    result.map(|uv| (uv, Outcome::Installed))
}

async fn install_into(
    release: &Release,
    asset: &Asset,
    base: &str,
    site: &Site<'_>,
    progress: &impl Fn(InstallProgress),
) -> Result<Uv, EngineInstallError> {
    let Site {
        root,
        staging,
        lock,
        landing,
    } = *site;
    let archive = staging.join(asset.file);
    download(
        &format!("{base}{}", asset.file),
        &archive,
        asset.sha256,
        asset.bytes,
        0,
        asset.bytes,
        progress,
    )
    .await?;

    progress(InstallProgress::Unpacking);
    let unpacked = staging.join("build");
    // The lock goes with each blocking step: an install dropped meanwhile
    // cannot stop a blocking task that has started, so none may begin until
    // it ends.
    let held = Arc::clone(lock);
    let (from, target, name) = (archive.clone(), unpacked.clone(), asset.file.to_owned());
    tokio::task::spawn_blocking(move || {
        let _held = held;
        let file = File::open(&from).map_err(|error| io_error(&from, error))?;
        super::fetch::unpack(GzDecoder::new(file), &name, &target)
    })
    .await
    .map_err(|error| io_error(&unpacked, error))??;

    progress(InstallProgress::Checking);
    let held = Arc::clone(lock);
    let record = Installed {
        version: release.version.to_owned(),
        os: asset.os.as_str().to_owned(),
        arch: asset.arch.to_owned(),
        sha256: asset.sha256.to_owned(),
        installed_at: kernel::time::now_millis(),
        files: Vec::new(),
    };
    let (root, staging) = (root.to_path_buf(), staging.to_path_buf());
    let checked = unpacked.clone();
    let dropped = Dropped::watching(landing);
    let commit = dropped.commit();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        finish(record, &root, &staging, &checked, &commit)
    })
    .await
    .map_err(|error| io_error(&unpacked, error))?
}

/// Put the version unpacked at `unpacked` in place under `root`, once its
/// `uv` says it is the version expected, unless the install was dropped
/// first (`commit`): write
/// its record, and move a broken install of the same version aside (put back
/// if the new one cannot take its place). Other versions stay: a build may be
/// running one, and the next lookup prefers this one anyway; `hedos engines
/// remove uv` removes them.
fn finish(
    mut record: Installed,
    root: &Path,
    staging: &Path,
    unpacked: &Path,
    commit: &Commit,
) -> Result<Uv, EngineInstallError> {
    let smoke = |found: String| EngineInstallError::Smoke {
        program: PROGRAM.to_owned(),
        expected: format!("{ENGINE} {}", record.version),
        found,
    };
    let said = ask_version(&unpacked.join(PROGRAM), staging).map_err(smoke)?;
    match parse_version(&said.stdout) {
        Some(version) if said.status.success() && version == record.version => {}
        Some(version) if said.status.success() => {
            return Err(smoke(format!("it is {ENGINE} {version}")));
        }
        _ => return Err(smoke(said.failure())),
    }
    record.files = inventory(unpacked)?;
    let name = record.version.clone();
    let destination = put_in_place(&record, ENGINE, unpacked, root, staging, &name, commit)?;
    Ok(Uv {
        path: destination.join(PROGRAM),
        from: Source::Hedos,
    })
}

/// Whether a model `runtime` serves needs uv before it can run: the runtime
/// builds its environment with uv, that environment is not built yet for the
/// lockfile there now, and no uv is found. A model whose environment is built
/// runs without uv, so removing uv never stops it.
pub fn needed(runtime: &RuntimeId) -> bool {
    needing([runtime]).contains(runtime)
}

/// Which of `runtimes` need uv before their models run ([`needed`]), asked
/// together: uv is looked for once, and the manifests read once, only when
/// none is found and one of them could be a manifest runtime.
pub fn needing<'a>(runtimes: impl IntoIterator<Item = &'a RuntimeId>) -> HashSet<RuntimeId> {
    needing_in(&HedosDirs::detect(), elsewhere, runtimes)
}

/// [`needing`] under `dirs`, with a uv not hedos's looked for by `elsewhere`.
fn needing_in<'a>(
    dirs: &HedosDirs,
    elsewhere: impl FnOnce() -> Option<PathBuf>,
    runtimes: impl IntoIterator<Item = &'a RuntimeId>,
) -> HashSet<RuntimeId> {
    if locate(&dirs.data, elsewhere).is_some() {
        return HashSet::new();
    }
    unbuilt_among(dirs, runtimes)
}

/// Which of `runtimes` build their environment with uv and have it unbuilt
/// for their current lockfile under `dirs`: a Python sidecar wired in code
/// (environments in `env/<name>`, lockfile in its bundle), or a manifest
/// runtime with `[env]` (environments in `env/manifests`). A sidecar whose
/// bundle is not unpacked yet has no environment either. Any other runtime
/// boot wires in code builds none, so the manifests are read only for a
/// runtime that is neither.
fn unbuilt_among<'a>(
    dirs: &HedosDirs,
    runtimes: impl IntoIterator<Item = &'a RuntimeId>,
) -> HashSet<RuntimeId> {
    let environments = dirs.sub("env");
    let sidecars = boot::python_sidecars();
    let built_in = boot::built_in_ids();
    let mut manifests = None;
    let mut unbuilt = HashSet::new();
    for runtime in runtimes {
        let sidecar = sidecars.iter().find(|(id, _)| id == runtime);
        let needs = if let Some((_, bundle)) = sidecar {
            let lockfile = RuntimeBundle::directory(bundle, &[dirs.sub("bundles")])
                .map(|bundle| bundle.join("requirements.lock"));
            lockfile.is_none_or(|lockfile| {
                !EnvironmentManager::new(environments.join(boot::sidecar_name(runtime)))
                    .is_built(runtime.as_str(), &lockfile)
            })
        } else if built_in.contains(runtime.as_str()) {
            false
        } else {
            let listing = manifests.get_or_insert_with(|| boot::manifest_listing(dirs, &built_in));
            listing
                .manifests
                .iter()
                .find(|manifest| manifest.id == runtime.as_str())
                .and_then(|manifest| Some((manifest, manifest.env.as_ref()?)))
                .is_some_and(|(manifest, env)| {
                    manifest.directory.as_ref().is_none_or(|directory| {
                        !EnvironmentManager::new(environments.join("manifests"))
                            .is_built(&manifest.id, &directory.join(&env.lockfile))
                    })
                })
        };
        if needs {
            unbuilt.insert(runtime.clone());
        }
    }
    unbuilt
}

/// Remove every uv version hedos installed under `data`, never a uv it did
/// not install; the paths removed.
pub fn remove(data: &Path) -> Result<Vec<PathBuf>, EngineInstallError> {
    let root = hedos_root(data);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let _lock = lock(&root, ENGINE)?;
    recover(&root)?;
    remove_installs(&root)
}

#[cfg(test)]
mod tests;
