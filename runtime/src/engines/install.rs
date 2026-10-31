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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flate2::read::GzDecoder;
use kernel::engines::llama_cpp::{
    Asset, INSTALLED_FILE, InstallSource, Installed, LlamaVersion, PINNED, Release, Variant,
    install_dir_name, loader_failure, parse_build_tag, parse_llama_version,
};
use kernel::machine::Os;
use sha2::{Digest, Sha256};

use super::bundled::{Bundled, bundled};
use super::fetch::{
    Commit, Dropped, Site, ask_version, create_dir_all, download, fresh_staging, installs,
    inventory, io_error, lock, lock_within, put_in_place, recover, remove_entry, remove_installs,
    try_lock, unpack,
};
use super::llama_cpp::{LlamaServer, Source, hedos_root};

pub use super::fetch::{EngineInstallError, InstallProgress, Landing};

/// The engine this module installs, as its errors name it.
const ENGINE: &str = "llama.cpp";
/// The program a build is checked with.
const PROGRAM: &str = "llama-server";
/// How long unpacking the carried build waits for another process holding
/// the lock before saying llama.cpp is being installed.
const UNPACK_WAIT: Duration = Duration::from_secs(60);

/// Install the pinned llama.cpp build of `variant` for this machine under
/// `data`, reporting through `progress`; `landing` says where it put the
/// build, for a caller that drops it (Ctrl-C) as it was finishing.
pub async fn install(
    variant: Variant,
    data: &Path,
    landing: &Landing,
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
        landing,
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
    landing: &Landing,
    progress: impl Fn(InstallProgress),
) -> Result<LlamaServer, EngineInstallError> {
    let root = hedos_root(data);
    create_dir_all(&root)?;
    // Taking the lock may wait a moment (`lock`), and putting right what an
    // interrupted install left can delete most of a gigabyte: neither may
    // stall this runtime's other tasks.
    let preparing = root.clone();
    let (lock, staging) = tokio::task::spawn_blocking(move || {
        let lock = lock(&preparing, ENGINE)?;
        recover(&preparing)?;
        let staging = fresh_staging(&preparing)?;
        Ok::<_, EngineInstallError>((lock, staging))
    })
    .await
    .map_err(|error| io_error(&root, error))??;
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
    result
}

async fn install_into(
    release: &Release,
    asset: &Asset,
    base: &str,
    site: &Site<'_>,
    progress: &impl Fn(InstallProgress),
) -> Result<LlamaServer, EngineInstallError> {
    let Site {
        root,
        staging,
        lock,
        landing,
    } = *site;
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
    let dropped = Dropped::watching(landing);
    let commit = dropped.commit();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        finish(&built, &root, &staging, &checked, &commit)
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
/// `llama-server` says it is the build expected, unless the install was
/// dropped first (`commit`):
/// write its record, move a build of the same name aside (put back if the new
/// one cannot take its place), and then remove the other builds of the same
/// source. A build from the other source is never touched: the one hedos
/// carries and one installed on request live side by side.
fn finish(
    built: &Built,
    root: &Path,
    staging: &Path,
    unpacked: &Path,
    commit: &Commit,
) -> Result<LlamaServer, EngineInstallError> {
    match smoke(&unpacked.join(PROGRAM), staging) {
        Ok(version) if Some(version.build) == built.build_number => {}
        Ok(version) => {
            return Err(EngineInstallError::Smoke {
                program: PROGRAM.to_owned(),
                expected: format!("llama.cpp {}", built.build),
                found: format!("it is {}", version.label()),
            });
        }
        Err(found) => {
            return Err(EngineInstallError::Smoke {
                program: PROGRAM.to_owned(),
                expected: format!("llama.cpp {}", built.build),
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
    let name = install_dir_name(&built.build, built.variant, built.source);
    let destination = put_in_place(&record, ENGINE, unpacked, root, staging, &name, commit)?;
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
        None => lock_within(&root, ENGINE, wait)?,
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
    finish(&built, root, staging, &unpacked, &Commit::undroppable())
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
    let _lock = lock(&root, ENGINE)?;
    recover(&root)?;
    remove_installs(&root)
}

/// The version the `llama-server` at `server` reports, or what went wrong
/// asking: it could not start, it did not answer in time, or its exit status
/// and the end of what it wrote (a library it could not load, say). What it
/// writes goes to files in `staging`.
fn smoke(server: &Path, staging: &Path) -> Result<LlamaVersion, String> {
    let said = ask_version(server, staging)?;
    match parse_llama_version(&said.stderr) {
        Some(version) if said.status.success() => Ok(version),
        _ => Err(loader_failure(&said.stderr).unwrap_or_else(|| said.failure())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::fetch::LOCK_FILE;
    use crate::engines::testing::{Item, Scratch, archive, serve};
    use kernel::engines::llama_cpp::Publisher;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Instant;

    use crate::engines::llama_cpp::{installed, locate};

    const RELEASE: Release = Release {
        version: "0.7.0",
        build: "b11669",
        source: PINNED.source,
        assets: &[],
    };

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

    async fn run(
        scratch: &Scratch,
        published: Vec<u8>,
        pinned: &Asset,
    ) -> Result<LlamaServer, EngineInstallError> {
        let (base, accept) = serve(HashMap::from([("build.tar.gz", published)])).await;
        let seen = Mutex::new(Vec::new());
        let result = install_from(
            &RELEASE,
            pinned,
            &base,
            &scratch.0,
            &Landing::default(),
            |event| {
                seen.lock().unwrap().push(event);
            },
        )
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
        let held = lock(&hedos_root(&scratch.0), ENGINE).unwrap();
        let published = good_build();
        let pinned = asset_of(&published, Variant::Metal);
        assert_eq!(
            run(&scratch, published, &pinned).await,
            Err(EngineInstallError::Busy {
                engine: "llama.cpp".to_owned()
            })
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
        let _lock = lock(&root, ENGINE).unwrap();
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
        assert!(matches!(
            lock(&root, ENGINE),
            Err(EngineInstallError::Io { .. })
        ));
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
        let _lock = lock(&root, ENGINE).unwrap();
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
        let held = lock(&root, ENGINE).unwrap();
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
        let held = lock(&root, ENGINE).unwrap();
        let started = Instant::now();
        let refused = unpack_bundle_within(
            &bundle_of(&good_build(), Variant::Metal),
            &RELEASE,
            &scratch.0,
            Duration::from_millis(200),
        );
        assert!(
            matches!(refused, Err(EngineInstallError::Busy { .. })),
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
        let held = lock(&root, ENGINE).unwrap();
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
        let server = install_from(
            &PINNED,
            asset,
            &base,
            &scratch.0,
            &Landing::default(),
            |_| {},
        )
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
