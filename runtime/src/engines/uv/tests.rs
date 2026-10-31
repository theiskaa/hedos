use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use kernel::engines::InstalledFile;
use sha2::{Digest, Sha256};

use super::*;
use crate::engines::fetch::{LOCK_FILE, STAGING_PREFIX};
use crate::engines::testing::{Item, Scratch, archive, serve};

const RELEASE: Release = Release {
    version: "0.13.4",
    assets: &[],
};

fn uv_script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'uv {version} (test)'\n").into_bytes()
}

fn release_of(version: &str) -> Vec<u8> {
    let uv = uv_script(version);
    archive(&[
        Item::Dir("uv-test/"),
        Item::File("uv-test/uv", &uv, 0o755),
        Item::File("uv-test/uvx", b"#!/bin/sh\n", 0o755),
    ])
}

/// An asset for `bytes` as published, its digest and size pinned.
fn asset_of(bytes: &[u8]) -> Asset {
    Asset {
        os: Os::current(),
        arch: std::env::consts::ARCH,
        file: "uv.tar.gz",
        sha256: Box::leak(hex::encode(Sha256::digest(bytes)).into_boxed_str()),
        bytes: bytes.len() as u64,
    }
}

async fn run(
    data: &Path,
    published: Option<Vec<u8>>,
    pinned: &Asset,
) -> Result<(Uv, Outcome), EngineInstallError> {
    run_seeing(data, published, pinned).await.0
}

/// [`run`], with the steps it reported, a download's once however many
/// chunks it came in.
async fn run_seeing(
    data: &Path,
    published: Option<Vec<u8>>,
    pinned: &Asset,
) -> (Result<(Uv, Outcome), EngineInstallError>, Vec<&'static str>) {
    let files = published
        .map(|bytes| HashMap::from([("uv.tar.gz", bytes)]))
        .unwrap_or_default();
    let (base, accept) = serve(files).await;
    let seen = Mutex::new(Vec::new());
    let result = install_from(
        &RELEASE,
        pinned,
        &base,
        data,
        &Landing::default(),
        |event| {
            let step = match event {
                InstallProgress::Downloading { .. } => "downloading",
                InstallProgress::Unpacking => "unpacking",
                InstallProgress::Checking => "checking",
            };
            let mut seen = seen.lock().unwrap();
            if seen.last() != Some(&step) {
                seen.push(step);
            }
        },
    )
    .await;
    accept.abort();
    (result, seen.into_inner().unwrap())
}

fn nothing_but_the_lock(data: &Path) -> bool {
    std::fs::read_dir(hedos_root(data))
        .unwrap()
        .flatten()
        .all(|entry| entry.file_name() == LOCK_FILE)
}

fn nowhere() -> Option<PathBuf> {
    None
}

/// A version under `data` as an install leaves it, installed at `at`.
fn put(data: &Path, version: &str, at: i64) -> PathBuf {
    put_from(data, version, at, &"0".repeat(64))
}

/// [`put`], recording the archive `sha256` it came from.
fn put_from(data: &Path, version: &str, at: i64, sha256: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = hedos_root(data).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let uv = uv_script(version);
    std::fs::write(dir.join(PROGRAM), &uv).unwrap();
    std::fs::set_permissions(dir.join(PROGRAM), std::fs::Permissions::from_mode(0o755)).unwrap();
    let record = Installed {
        version: version.to_owned(),
        os: Os::current().as_str().to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        sha256: sha256.to_owned(),
        installed_at: at,
        files: vec![InstalledFile {
            name: PROGRAM.to_owned(),
            bytes: Some(uv.len() as u64),
        }],
    };
    std::fs::write(
        dir.join(INSTALLED_FILE),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn the_pinned_uv_installs_whole_and_is_the_one_located() {
    let scratch = Scratch::new("uv-good");
    let published = release_of("0.13.4");
    let pinned = asset_of(&published);
    let (result, seen) = run_seeing(&scratch.0, Some(published), &pinned).await;
    let (uv, outcome) = result.expect("installed");
    assert_eq!(seen, ["downloading", "unpacking", "checking"]);

    let dir = hedos_root(&scratch.0).join("0.13.4");
    assert_eq!(outcome, Outcome::Installed);
    assert_eq!(uv.path, dir.join("uv"));
    assert_eq!(uv.from, Source::Hedos);
    assert!(kernel::fs::is_executable(&dir.join("uvx")));
    let (found, record) = installed(&scratch.0).expect("an install");
    assert_eq!(found, dir);
    assert_eq!(record.sha256, pinned.sha256);
    let names: Vec<&str> = record.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, ["uv", "uvx"]);
    assert_eq!(locate(&scratch.0, nowhere), Some(uv.clone()));
    assert_eq!(version(&uv.path).as_deref(), Some("0.13.4"));
    assert!(nothing_hidden_but_the_lock(&scratch.0));

    // Asked again, nothing is fetched: there is nothing to fetch from. What
    // an install stopped after its last step left is cleared all the same.
    let leftover = hedos_root(&scratch.0).join(format!("{STAGING_PREFIX}1-1"));
    std::fs::create_dir_all(leftover.join("build")).unwrap();
    let (again, outcome) = run(&scratch.0, None, &pinned).await.expect("present");
    assert_eq!(outcome, Outcome::Present);
    assert_eq!(again, uv);
    assert!(!leftover.exists());
}

fn nothing_hidden_but_the_lock(data: &Path) -> bool {
    std::fs::read_dir(hedos_root(data))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
        .all(|entry| entry.file_name() == LOCK_FILE)
}

#[tokio::test]
async fn an_archive_that_is_not_the_one_pinned_installs_nothing() {
    let scratch = Scratch::new("uv-digest");
    let pinned = asset_of(&release_of("0.13.4"));
    let tampered = release_of("0.13.5");
    let mut sized = pinned;
    sized.bytes = tampered.len() as u64;
    let refused = run(&scratch.0, Some(tampered), &sized).await;
    assert!(
        matches!(refused, Err(EngineInstallError::Checksum { .. })),
        "{refused:?}"
    );
    assert!(nothing_but_the_lock(&scratch.0));
    assert_eq!(installed(&scratch.0), None);
}

#[tokio::test]
async fn an_archive_larger_than_pinned_is_cut_off() {
    let scratch = Scratch::new("uv-size");
    let published = release_of("0.13.4");
    let mut pinned = asset_of(&published);
    pinned.bytes -= 1;
    let refused = run(&scratch.0, Some(published), &pinned).await;
    assert!(
        matches!(refused, Err(EngineInstallError::Download { ref reason, .. }) if reason.contains("larger")),
        "{refused:?}"
    );
    assert!(nothing_but_the_lock(&scratch.0));
}

#[tokio::test]
async fn an_entry_reaching_outside_the_archive_refuses_it() {
    let scratch = Scratch::new("uv-hostile");
    let uv = uv_script("0.13.4");
    let published = archive(&[
        Item::File("uv-test/uv", &uv, 0o755),
        Item::File("uv-test/../../escaped", b"x", 0o644),
    ]);
    let pinned = asset_of(&published);
    let refused = run(&scratch.0, Some(published), &pinned).await;
    assert!(
        matches!(refused, Err(EngineInstallError::Archive { .. })),
        "{refused:?}"
    );
    assert!(nothing_but_the_lock(&scratch.0));
    assert!(!scratch.0.join("escaped").exists());
}

#[tokio::test]
async fn a_uv_that_says_another_version_is_not_put_in_place() {
    let scratch = Scratch::new("uv-version");
    let published = release_of("0.13.3");
    let pinned = asset_of(&published);
    match run(&scratch.0, Some(published), &pinned).await {
        Err(EngineInstallError::Smoke {
            program,
            expected,
            found,
        }) => {
            assert_eq!((program.as_str(), expected.as_str()), ("uv", "uv 0.13.4"));
            assert!(found.contains("0.13.3"), "{found}");
        }
        other => panic!("{other:?}"),
    }
    assert!(nothing_but_the_lock(&scratch.0));
}

#[tokio::test]
async fn a_broken_install_of_the_pinned_version_is_replaced_and_others_stay() {
    let scratch = Scratch::new("uv-broken");
    let published = release_of("0.13.4");
    let pinned = asset_of(&published);
    // From the very archive pinned, so only its being broken has it fetched.
    let pinned_dir = put_from(&scratch.0, "0.13.4", 2, pinned.sha256);
    // Cut short since: no longer the size its record says.
    std::fs::write(pinned_dir.join(PROGRAM), b"#!/bin/sh\n").unwrap();
    assert_eq!(installed(&scratch.0).map(|(dir, _)| dir), None);
    let older = put(&scratch.0, "0.12.0", 1);
    // What an install that never finished left.
    let leftover = hedos_root(&scratch.0).join(format!("{STAGING_PREFIX}1-1"));
    std::fs::create_dir_all(leftover.join("build")).unwrap();

    let (uv, outcome) = run(&scratch.0, Some(published), &pinned)
        .await
        .expect("installed");
    assert_eq!(outcome, Outcome::Installed);
    assert_eq!(uv.path, pinned_dir.join("uv"));
    assert_eq!(version(&uv.path).as_deref(), Some("0.13.4"));
    // A build may be running another version; only a removal takes it.
    assert!(older.join("uv").exists());
    assert!(!leftover.exists());
}

#[test]
fn hedos_s_own_uv_comes_first_the_pinned_one_before_the_last_installed() {
    let scratch = Scratch::new("uv-order");
    let theirs = scratch.0.join("bin/uv");
    let elsewhere = || Some(theirs.clone());
    assert_eq!(
        locate(&scratch.0, elsewhere),
        Some(Uv {
            path: theirs.clone(),
            from: Source::Path,
        })
    );
    assert_eq!(locate(&scratch.0, nowhere), None);

    let older = put(&scratch.0, "0.12.0", 1);
    assert_eq!(
        locate(&scratch.0, elsewhere).map(|uv| uv.path),
        Some(older.join("uv"))
    );
    let newer = put(&scratch.0, "0.14.0", 3);
    assert_eq!(installed(&scratch.0).map(|(dir, _)| dir), Some(newer));
    let pinned = put(&scratch.0, "0.13.4", 2);
    assert_eq!(
        locate(&scratch.0, elsewhere),
        Some(Uv {
            path: pinned.join("uv"),
            from: Source::Hedos,
        })
    );
}

#[test]
fn only_a_directory_named_for_its_record_counts() {
    let scratch = Scratch::new("uv-named");
    let real = put(&scratch.0, "0.13.4", 1);
    let renamed = hedos_root(&scratch.0).join("latest");
    std::fs::rename(&real, &renamed).unwrap();
    assert_eq!(installed(&scratch.0), None);
    // A link to an install elsewhere is not one of hedos's own.
    let outside = Scratch::new("uv-named-outside");
    let elsewhere = put(&outside.0, "0.13.4", 1);
    std::os::unix::fs::symlink(&elsewhere, hedos_root(&scratch.0).join("0.13.4")).unwrap();
    assert_eq!(installed(&scratch.0), None);
}

#[test]
fn removing_takes_every_version_hedos_installed_and_only_those() {
    let scratch = Scratch::new("uv-remove");
    assert_eq!(remove(&scratch.0), Ok(Vec::new()));
    let pinned = put(&scratch.0, "0.13.4", 2);
    let older = put(&scratch.0, "0.12.0", 1);
    let theirs = scratch.0.join("bin/uv");
    std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
    std::fs::write(&theirs, b"#!/bin/sh\n").unwrap();

    let mut removed = remove(&scratch.0).expect("removed");
    removed.sort();
    assert_eq!(removed, [older, pinned]);
    assert!(nothing_but_the_lock(&scratch.0));
    assert!(theirs.exists());
    assert_eq!(locate(&scratch.0, nowhere), None);
}

/// How a pin's archive for this machine is checked: installed through the
/// download path, it is the size and digest pinned, and its `uv` says it is
/// the version pinned.
#[tokio::test]
#[ignore = "needs HEDOS_PINNED_UV_ARCHIVE, the pinned uv archive for this machine"]
async fn the_pinned_archive_installs_and_reports_the_pinned_version() {
    let path = std::env::var("HEDOS_PINNED_UV_ARCHIVE").expect("HEDOS_PINNED_UV_ARCHIVE");
    let published = std::fs::read(&path).expect("the archive");
    let asset = PINNED
        .asset(Os::current(), std::env::consts::ARCH)
        .expect("a build for this machine");
    assert_eq!(hex::encode(Sha256::digest(&published)), asset.sha256);
    let scratch = Scratch::new("uv-pinned-real");
    let (base, accept) = serve(HashMap::from([(asset.file, published)])).await;
    let (uv, outcome) = install_from(
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
    assert_eq!(outcome, Outcome::Installed);
    assert_eq!(version(&uv.path).as_deref(), Some(PINNED.version));
    let (_, record) = installed(&scratch.0).expect("an install");
    let names: Vec<&str> = record.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, ["uv", "uvx"]);
}

/// A data dir with a runtime's lockfile at `lockfile`, so environments can be
/// marked built for it.
fn mark_built(environments: &Path, runtime: &str, lockfile: &Path) {
    let hash = EnvironmentManager::lock_hash(lockfile).unwrap();
    let env = environments
        .join("runtimes")
        .join(crate::environment::sanitized_runtime_id(runtime))
        .join("envs")
        .join(hash);
    std::fs::create_dir_all(&env).unwrap();
    std::fs::write(env.join(".hedos-env-ok"), b"").unwrap();
    assert!(EnvironmentManager::new(environments.to_path_buf()).is_built(runtime, lockfile));
}

#[test]
fn a_python_sidecar_needs_uv_until_its_environment_is_built() {
    let scratch = Scratch::new("uv-needed-sidecar");
    let dirs = HedosDirs {
        data: scratch.0.clone(),
    };
    let mlx = RuntimeId::mlx_lm();
    // No bundle unpacked yet: nothing built either.
    assert!(unbuilt_one(&dirs, &mlx));
    let bundle = scratch.0.join("bundles/Runtimes/python-mlx-lm");
    std::fs::create_dir_all(&bundle).unwrap();
    let lockfile = bundle.join("requirements.lock");
    std::fs::write(&lockfile, "mlx-lm==1 --hash=sha256:00").unwrap();
    assert!(unbuilt_one(&dirs, &mlx));
    mark_built(&scratch.0.join("env/mlx-lm"), "python:mlx-lm", &lockfile);
    assert!(!unbuilt_one(&dirs, &mlx));
    // A changed lock has no environment built for it.
    std::fs::write(&lockfile, "mlx-lm==2 --hash=sha256:01").unwrap();
    assert!(unbuilt_one(&dirs, &mlx));

    // Audio generation is wired in code too, its environments in env/audio.
    let audio = RuntimeId::audio();
    let bundle = scratch.0.join("bundles/Runtimes/python-audio");
    std::fs::create_dir_all(&bundle).unwrap();
    let lockfile = bundle.join("requirements.lock");
    std::fs::write(&lockfile, "torch==2 --hash=sha256:00").unwrap();
    mark_built(&scratch.0.join("env/manifests"), "python:audio", &lockfile);
    assert!(unbuilt_one(&dirs, &audio));
    mark_built(&scratch.0.join("env/audio"), "python:audio", &lockfile);
    assert!(!unbuilt_one(&dirs, &audio));

    assert!(!unbuilt_one(&dirs, &RuntimeId::llama_cpp()));
}

#[test]
fn a_manifest_runtime_with_an_environment_needs_uv_until_it_is_built() {
    let scratch = Scratch::new("uv-needed-manifest");
    let dirs = HedosDirs {
        data: scratch.0.clone(),
    };
    let write = |name: &str, body: &str| {
        let dir = scratch.0.join("runtimes.d").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.toml"), body).unwrap();
        std::fs::write(dir.join("main.py"), "").unwrap();
        dir
    };
    let with_env = write(
        "mine",
        "id = \"acme:thing\"\nmodalities = [\"text\"]\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n\n[env]\nlockfile = \"requirements.lock\"\n\n[serve]\nentrypoint = \"main.py\"\nprotocol = \"ndjson+frames\"\n",
    );
    let lockfile = with_env.join("requirements.lock");
    std::fs::write(&lockfile, "x==1 --hash=sha256:00").unwrap();
    write(
        "plain",
        "id = \"acme:plain\"\nmodalities = [\"text\"]\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n\n[serve]\nentrypoint = \"main.py\"\nprotocol = \"ndjson+frames\"\n",
    );
    let thing = RuntimeId::from("acme:thing");
    assert!(unbuilt_one(&dirs, &thing));
    assert!(!unbuilt_one(&dirs, &RuntimeId::from("acme:plain")));
    assert!(!unbuilt_one(&dirs, &RuntimeId::from("acme:absent")));
    mark_built(&scratch.0.join("env/manifests"), "acme:thing", &lockfile);
    assert!(!unbuilt_one(&dirs, &thing));
}

#[test]
fn present_puts_right_what_an_interrupted_install_left() {
    let scratch = Scratch::new("uv-present");
    assert_eq!(present(&scratch.0), None);
    let dir = put(&scratch.0, PINNED.version, 1);
    // Not the archive pinned for this machine: someone else's 0.13.4.
    assert_eq!(present(&scratch.0), None);
    let asset = PINNED
        .asset(Os::current(), std::env::consts::ARCH)
        .expect("a build for this machine");
    let record_path = dir.join(INSTALLED_FILE);
    let mut record: Installed =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record.sha256 = asset.sha256.to_owned();
    std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
    let leftover = hedos_root(&scratch.0).join(format!("{STAGING_PREFIX}1-1"));
    std::fs::create_dir_all(leftover.join("build")).unwrap();
    assert_eq!(
        present(&scratch.0),
        Some(Uv {
            path: dir.join("uv"),
            from: Source::Hedos,
        })
    );
    assert!(!leftover.exists());
}

fn unbuilt_one(dirs: &HedosDirs, runtime: &RuntimeId) -> bool {
    unbuilt_among(dirs, [runtime]).contains(runtime)
}

#[test]
fn runtimes_asked_together_get_the_same_answers_as_one_by_one() {
    let scratch = Scratch::new("uv-needed-together");
    let dirs = HedosDirs {
        data: scratch.0.clone(),
    };
    let dir = scratch.0.join("runtimes.d/mine");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        "id = \"acme:thing\"\nmodalities = [\"text\"]\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n\n[env]\nlockfile = \"requirements.lock\"\n\n[serve]\nentrypoint = \"main.py\"\nprotocol = \"ndjson+frames\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("main.py"), "").unwrap();
    std::fs::write(dir.join("requirements.lock"), "x==1 --hash=sha256:00").unwrap();
    let asked = [
        RuntimeId::mlx_lm(),
        RuntimeId::llama_cpp(),
        RuntimeId::ollama(),
        RuntimeId::from("acme:thing"),
        RuntimeId::from("acme:absent"),
    ];
    let together = unbuilt_among(&dirs, &asked);
    for runtime in &asked {
        assert_eq!(
            together.contains(runtime),
            unbuilt_one(&dirs, runtime),
            "{runtime:?}"
        );
    }
    assert_eq!(
        together,
        HashSet::from([RuntimeId::mlx_lm(), RuntimeId::from("acme:thing")])
    );
}

#[tokio::test]
async fn a_pinned_install_damaged_without_changing_size_is_fetched_again() {
    let scratch = Scratch::new("uv-damaged");
    let published = release_of("0.13.4");
    let pinned = asset_of(&published);
    let dir = put_from(&scratch.0, "0.13.4", 1, pinned.sha256);
    let program = dir.join(PROGRAM);
    let length = std::fs::metadata(&program).unwrap().len() as usize;
    let mut damaged = b"#!/bin/sh\nexit 1\n".to_vec();
    damaged.resize(length, b'#');
    std::fs::write(&program, &damaged).unwrap();
    // Whole by its record, so it is still the one located.
    assert_eq!(
        installed(&scratch.0).map(|(found, _)| found),
        Some(dir.clone())
    );

    let (uv, outcome) = run(&scratch.0, Some(published), &pinned)
        .await
        .expect("installed");
    assert_eq!(outcome, Outcome::Installed);
    assert_eq!(uv.path, program);
    assert_eq!(version(&program).as_deref(), Some("0.13.4"));
}

#[test]
fn an_install_for_another_machine_is_not_located() {
    let scratch = Scratch::new("uv-platform");
    let dir = put(&scratch.0, PINNED.version, 1);
    let record_path = dir.join(INSTALLED_FILE);
    let mut record: Installed =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    let here = record.clone();
    record.arch = "riscv64".to_owned();
    std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(installed(&scratch.0), None);
    assert_eq!(locate(&scratch.0, nowhere), None);
    // This architecture, another system.
    record = here;
    record.os = "freebsd".to_owned();
    std::fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(installed(&scratch.0), None);
}

#[test]
fn present_asks_the_pinned_uv_its_version() {
    let scratch = Scratch::new("uv-present-damaged");
    let asset = PINNED
        .asset(Os::current(), std::env::consts::ARCH)
        .expect("a build for this machine");
    let dir = put_from(&scratch.0, PINNED.version, 1, asset.sha256);
    assert_eq!(present(&scratch.0).map(|uv| uv.path), Some(dir.join("uv")));
    let program = dir.join(PROGRAM);
    let length = std::fs::metadata(&program).unwrap().len() as usize;
    let mut damaged = b"#!/bin/sh\nexit 1\n".to_vec();
    damaged.resize(length, b'#');
    std::fs::write(&program, &damaged).unwrap();
    assert!(installed(&scratch.0).is_some());
    assert_eq!(present(&scratch.0), None);
}

#[tokio::test]
async fn a_uv_that_fails_says_why_in_the_error() {
    let scratch = Scratch::new("uv-stderr");
    let script = b"#!/bin/sh\necho 'cannot load libfoo' >&2\nexit 3\n";
    let published = archive(&[
        Item::Dir("uv-test/"),
        Item::File("uv-test/uv", script, 0o755),
    ]);
    let pinned = asset_of(&published);
    match run(&scratch.0, Some(published), &pinned).await {
        Err(EngineInstallError::Smoke { found, .. }) => {
            assert!(found.contains("cannot load libfoo"), "{found}");
        }
        other => panic!("{other:?}"),
    }
    assert!(nothing_but_the_lock(&scratch.0));
}

#[test]
fn an_install_dropped_while_checking_puts_nothing_in_place() {
    let scratch = Scratch::new("uv-stopped");
    let root = hedos_root(&scratch.0);
    let staging = root.join(format!("{STAGING_PREFIX}1-1"));
    let unpacked = staging.join("build");
    std::fs::create_dir_all(&unpacked).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(unpacked.join(PROGRAM), uv_script("0.13.4")).unwrap();
        std::fs::set_permissions(
            unpacked.join(PROGRAM),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let record = Installed {
        version: "0.13.4".to_owned(),
        os: Os::current().as_str().to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        sha256: "0".repeat(64),
        installed_at: 1,
        files: Vec::new(),
    };
    let dropped = Dropped::watching(&Landing::default());
    let commit = dropped.commit();
    drop(dropped);
    assert_eq!(
        finish(record, &root, &staging, &unpacked, &commit).map(|uv| uv.path),
        Err(EngineInstallError::Stopped {
            engine: "uv".to_owned()
        })
    );
    assert!(!root.join("0.13.4").exists());
    assert_eq!(installed(&scratch.0), None);
}

#[test]
fn an_install_cut_off_between_its_moves_is_put_right() {
    let scratch = Scratch::new("uv-interrupted");
    let root = hedos_root(&scratch.0);
    // The broken install was set aside and the new one never took its place.
    let set_aside = put(&scratch.0, "0.12.0", 1);
    let staging = root.join(format!("{STAGING_PREFIX}1-1"));
    std::fs::create_dir_all(staging.join("replaced")).unwrap();
    std::fs::rename(&set_aside, staging.join("replaced/0.12.0")).unwrap();
    assert_eq!(installed(&scratch.0), None);

    assert_eq!(present(&scratch.0), None);
    assert!(!staging.exists());
    assert_eq!(installed(&scratch.0).map(|(dir, _)| dir), Some(set_aside));
}

#[test]
fn nothing_needs_uv_while_one_is_found() {
    let scratch = Scratch::new("uv-needing");
    let dirs = HedosDirs {
        data: scratch.0.clone(),
    };
    let asked = [RuntimeId::mlx_lm()];
    let needing = |elsewhere: Option<PathBuf>| needing_in(&dirs, || elsewhere, &asked);
    assert_eq!(needing(None), HashSet::from([RuntimeId::mlx_lm()]));
    assert!(needing(Some(scratch.0.join("bin/uv"))).is_empty());
    put(&scratch.0, "0.13.4", 1);
    assert!(needing(None).is_empty());
}

#[tokio::test]
async fn an_install_stopped_while_its_uv_is_asked_puts_nothing_in_place() {
    let scratch = Scratch::new("uv-abort");
    let script = b"#!/bin/sh\nsleep 1\necho 'uv 0.13.4 (test)'\n";
    let published = archive(&[
        Item::Dir("uv-test/"),
        Item::File("uv-test/uv", script, 0o755),
    ]);
    let pinned = asset_of(&published);
    let (base, accept) = serve(HashMap::from([("uv.tar.gz", published)])).await;
    let checking = Arc::new(tokio::sync::Notify::new());
    let landing = Landing::default();
    let installing = tokio::spawn({
        let (checking, data, landing) = (Arc::clone(&checking), scratch.0.clone(), landing.clone());
        async move {
            install_from(&RELEASE, &pinned, &base, &data, &landing, |event| {
                if event == InstallProgress::Checking {
                    checking.notify_one();
                }
            })
            .await
        }
    });
    checking.notified().await;
    installing.abort();
    // The check already running goes on until its uv answers; the lock it
    // holds is free once it has ended.
    let root = hedos_root(&scratch.0);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while try_lock(&root).unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the check never ended"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    accept.abort();
    assert!(!root.join("0.13.4").exists());
    assert_eq!(installed(&scratch.0), None);
    assert_eq!(landing.landed(), None);
}

#[tokio::test]
async fn an_install_says_where_it_landed() {
    let scratch = Scratch::new("uv-landing");
    let published = release_of("0.13.4");
    let pinned = asset_of(&published);
    let (base, accept) = serve(HashMap::from([("uv.tar.gz", published)])).await;
    let landing = Landing::default();
    let (uv, _) = install_from(&RELEASE, &pinned, &base, &scratch.0, &landing, |_| {})
        .await
        .expect("installed");
    accept.abort();
    assert_eq!(landing.landed(), uv.path.parent().map(Path::to_path_buf));
}
