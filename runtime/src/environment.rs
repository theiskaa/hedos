//! Python environment provisioning: one virtual environment per runtime, keyed
//! by the SHA-256 of its lockfile so a changed lock rebuilds and an unchanged
//! one short-circuits to a relink. The build itself is injectable (the default
//! shells out to the `uv` [`crate::engines::uv::located`] finds), and
//! honest-failure process running lives here too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernel::engines::uv::{TORCH_BACKEND_SINCE, takes_torch_backend};
use sha2::{Digest, Sha256};

use crate::governor::{BoxFuture, lock};
use crate::process::{MAX_BYTES_PER_STREAM, drain_bounded, terminate_tree};

/// Errors from environment provisioning.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    /// A filesystem operation failed.
    #[error("environment io error: {0}")]
    Io(#[from] std::io::Error),
    /// A build or subprocess step failed.
    #[error("{0}")]
    RuntimeFailed(String),
    /// A required tool (e.g. `uv`) is missing.
    #[error("{0}")]
    RuntimeUnavailable(String),
}

/// A progress reporter passed through a build.
pub type Progress = Arc<dyn Fn(&str) + Send + Sync>;

/// What one environment build is given.
pub struct Build {
    /// Where the environment goes.
    pub env_dir: PathBuf,
    /// The lockfile it installs exactly.
    pub lockfile: PathBuf,
    /// The package cache the manager's environments share.
    pub cache_dir: PathBuf,
    /// Where a Python the build has to download is kept, shared by every
    /// manager under one environments directory.
    pub python_dir: PathBuf,
    /// Whether the lockfile is a CPU variant (see
    /// [`EnvironmentManager::new`]), whose torch comes from PyTorch's CPU
    /// builds.
    pub cpu_torch: bool,
    /// Where each step is reported.
    pub progress: Progress,
}

/// Builds the environment [`Build`] describes.
pub type Builder = Arc<dyn Fn(Build) -> BoxFuture<Result<(), EnvError>> + Send + Sync>;

const ENV_MARKER: &str = ".hedos-env-ok";
const STDERR_TAIL_BYTES: usize = 400;
const DEFAULT_PROCESS_TIMEOUT: Duration = Duration::from_secs(900);
/// Fetching a lock's packages can take long: PyTorch's CUDA build and its
/// libraries are several GB. uv gives up on a stalled connection itself, so
/// this only bounds a fetch that never ends.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

struct Inner {
    root: PathBuf,
    builder: Builder,
    cpu_only: fn() -> bool,
    key_locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Provisions and caches per-runtime Python environments. Cheap to clone (an
/// `Arc` handle).
#[derive(Clone)]
pub struct EnvironmentManager {
    inner: Arc<Inner>,
}

impl EnvironmentManager {
    /// A manager rooted at `root`, building environments with the default `uv`
    /// builder.
    ///
    /// A lockfile with a CPU variant beside it (`requirements.cpu.lock` for
    /// `requirements.lock`) is one whose torch is PyPI's: CUDA on Linux, Metal
    /// on a Mac. On Linux without NVIDIA's driver the variant, with PyTorch's
    /// CPU build, is installed in its place.
    pub fn new(root: PathBuf) -> Self {
        Self::with_builder(root, Arc::new(uv_builder))
    }

    /// A manager rooted at `root` that builds environments with `builder`
    /// (injected for tests).
    pub fn with_builder(root: PathBuf, builder: Builder) -> Self {
        Self::seeing(root, builder, crate::machine::torch_on_the_cpu)
    }

    /// A manager that asks `cpu_only` whether torch runs on the CPU here, in
    /// place of reading the machine.
    pub(crate) fn seeing(root: PathBuf, builder: Builder, cpu_only: fn() -> bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                builder,
                cpu_only,
                key_locks: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// The lockfile an environment for `lockfile` installs on this machine:
    /// its CPU variant where torch runs on the CPU and one is shipped, else
    /// `lockfile`.
    fn lock_here(&self, lockfile: &Path) -> Lock {
        match cpu_variant(lockfile).filter(|variant| variant.is_file()) {
            Some(variant) if (self.inner.cpu_only)() => Lock {
                path: variant,
                cpu_torch: true,
            },
            _ => Lock {
                path: lockfile.to_owned(),
                cpu_torch: false,
            },
        }
    }

    /// Whether the environment for `runtime_id` at the current `lockfile` (or
    /// its CPU variant, as [`prepare`](Self::prepare) chooses) is built:
    /// `prepare` would return it without building.
    pub(crate) fn is_built(&self, runtime_id: &str, lockfile: &Path) -> bool {
        Self::lock_hash(&self.lock_here(lockfile).path).is_ok_and(|hash| {
            self.inner
                .root
                .join("runtimes")
                .join(sanitized_runtime_id(runtime_id))
                .join("envs")
                .join(hash)
                .join(ENV_MARKER)
                .exists()
        })
    }

    /// Where a Python uv downloads is kept: `python` beside this manager's
    /// root, so the managers under one environments directory (`env/mlx-lm`,
    /// `env/manifests`, ...) share it (`env/python`).
    pub(crate) fn python_dir(&self) -> PathBuf {
        match self.inner.root.parent() {
            Some(parent) => parent.join("python"),
            None => self.inner.root.join("python"),
        }
    }

    /// The first 8 bytes of `lockfile`'s SHA-256, hex-encoded — the environment
    /// cache key's hash component.
    pub fn lock_hash(lockfile: &Path) -> Result<String, EnvError> {
        let bytes = std::fs::read(lockfile)?;
        let digest = Sha256::digest(&bytes);
        Ok(digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    /// Ensure the environment for `runtime_id` at the current `lockfile` (or
    /// its CPU variant, [`lock_here`](Self::lock_here)) exists, building it if
    /// missing, and return its directory. Concurrent prepares for the same
    /// runtime+lock serialize; a build runs at most once (a later caller sees
    /// the marker and only relinks). A failed build leaves no marker, so it
    /// can be retried.
    pub async fn prepare(
        &self,
        runtime_id: &str,
        lockfile: &Path,
        progress: Progress,
    ) -> Result<PathBuf, EnvError> {
        let lock_here = self.lock_here(lockfile);
        let hash = Self::lock_hash(&lock_here.path)?;
        let key = format!("{runtime_id}#{hash}");
        let key_lock = {
            let mut locks = lock(&self.inner.key_locks);
            Arc::clone(locks.entry(key).or_default())
        };
        let _guard = key_lock.lock().await;
        self.perform_prepare(runtime_id, &hash, lock_here, progress)
            .await
    }

    async fn perform_prepare(
        &self,
        runtime_id: &str,
        hash: &str,
        lock_here: Lock,
        progress: Progress,
    ) -> Result<PathBuf, EnvError> {
        let safe_id = sanitized_runtime_id(runtime_id);
        let runtime_dir = self.inner.root.join("runtimes").join(&safe_id);
        let env_dir = runtime_dir.join("envs").join(hash);
        let current = runtime_dir.join("current");
        let marker = env_dir.join(ENV_MARKER);

        if marker.exists() {
            relink(&current, &env_dir)?;
            return Ok(env_dir);
        }

        progress("Preparing runtime…");
        if env_dir.exists() {
            std::fs::remove_dir_all(&env_dir)?;
        }
        if let Some(parent) = env_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cache_dir = self.inner.root.join("uv-cache");
        std::fs::create_dir_all(&cache_dir)?;

        (self.inner.builder)(Build {
            env_dir: env_dir.clone(),
            lockfile: lock_here.path,
            cache_dir,
            python_dir: self.python_dir(),
            cpu_torch: lock_here.cpu_torch,
            progress: Arc::clone(&progress),
        })
        .await?;

        std::fs::write(&marker, [])?;
        relink(&current, &env_dir)?;
        Ok(env_dir)
    }
}

/// The lockfile an environment installs on this machine.
#[derive(Debug, PartialEq, Eq)]
struct Lock {
    path: PathBuf,
    cpu_torch: bool,
}

/// The CPU variant of `lockfile`: `requirements.cpu.lock` for
/// `requirements.lock`.
pub(crate) fn cpu_variant(lockfile: &Path) -> Option<PathBuf> {
    let stem = lockfile.file_stem()?.to_str()?;
    let extension = lockfile.extension()?.to_str()?;
    Some(lockfile.with_file_name(format!("{stem}.cpu.{extension}")))
}

/// Environment keys always forwarded to a spawned runtime: process essentials and
/// locale/proxy/temp settings that affect correctness but carry no credentials.
/// Everything else is dropped; a runtime's real needs come through `overrides`.
/// Deliberately excludes `PYTHONPATH`/`PYTHONHOME` so a user's shell Python config
/// can't leak into the venv.
const BASELINE_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LC_ALL",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

#[cfg(target_os = "linux")]
const OS_BASELINE_KEYS: &[&str] = &["LD_LIBRARY_PATH"];

#[cfg(target_os = "macos")]
const OS_BASELINE_KEYS: &[&str] = &["DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"];

#[cfg(target_os = "windows")]
const OS_BASELINE_KEYS: &[&str] = &["USERPROFILE", "SystemRoot"];

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const OS_BASELINE_KEYS: &[&str] = &[];

/// The full set of allowlisted environment keys: [`BASELINE_KEYS`] plus the
/// current OS's additions.
fn baseline_keys() -> Vec<&'static str> {
    BASELINE_KEYS
        .iter()
        .copied()
        .chain(OS_BASELINE_KEYS.iter().copied())
        .collect()
}

/// The child environment for a sidecar or build: an allowlist of process
/// essentials and locale/proxy/temp settings from `base` (so unrelated
/// credentials in the parent environment — cloud keys, tokens, arbitrary shell
/// secrets — are not forwarded), then `overrides` applied on top. `overrides`
/// always wins, even for keys outside the allowlist, since that's how a runtime
/// declares what it actually needs.
pub fn scrubbed_environment<I>(
    base: I,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let allowed: std::collections::HashSet<&str> = baseline_keys().into_iter().collect();
    let mut env: BTreeMap<String, String> = base
        .into_iter()
        .filter(|(key, _)| allowed.contains(key.as_str()))
        .collect();
    for (key, value) in overrides {
        env.insert(key.clone(), value.clone());
    }
    env
}

/// The directory name a runtime's environments are kept under.
pub(crate) fn sanitized_runtime_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn relink(link: &Path, destination: &Path) -> Result<(), EnvError> {
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(link)?;
    }
    symlink(destination, link)
}

#[cfg(unix)]
fn symlink(destination: &Path, link: &Path) -> Result<(), EnvError> {
    std::os::unix::fs::symlink(destination, link).map_err(EnvError::from)
}

#[cfg(not(unix))]
fn symlink(_destination: &Path, _link: &Path) -> Result<(), EnvError> {
    Err(EnvError::RuntimeUnavailable(
        "symlinks are only supported on unix".to_owned(),
    ))
}

/// Run `executable` with `args`, draining its (bounded) stderr while racing a
/// wall-clock `timeout`. A timeout terminates the process tree and returns a
/// `"… timed out after Ns: <tail>"` error; a nonzero exit returns `"… failed:
/// <tail>"`. `extra_env` is layered over the scrubbed process environment.
pub async fn run_process(
    executable: &Path,
    args: &[&str],
    extra_env: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<(), EnvError> {
    let mut command = tokio::process::Command::new(executable);
    command.args(args);
    command.env_clear();
    for (key, value) in scrubbed_environment(std::env::vars(), extra_env) {
        command.env(key, value);
    }
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::piped());
    command.process_group(0);

    let mut child = command.spawn()?;
    // Drain stderr in its own task so the timeout below covers the whole run,
    // including the final wait: a child that closes stderr but keeps running
    // can't slip past the deadline. The drain finishes on stderr EOF, which the
    // process's exit (or our kill) always produces.
    let drain = tokio::spawn(drain_stderr(child.stderr.take()));

    let mut expired = false;
    let status = tokio::select! {
        status = child.wait() => status,
        _ = tokio::time::sleep(timeout) => {
            expired = true;
            terminate_tree(&mut child, Duration::from_secs(5)).await;
            child.wait().await
        }
    };
    let stderr_bytes = drain.await.unwrap_or_default();

    let tail = tail_string(&stderr_bytes, STDERR_TAIL_BYTES);
    let command_name = format!(
        "{} {}",
        executable
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        args.first().copied().unwrap_or_default(),
    );
    if expired {
        return Err(EnvError::RuntimeFailed(format!(
            "{command_name} timed out after {}s: {tail}",
            timeout.as_secs()
        )));
    }
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => Err(EnvError::RuntimeFailed(format!(
            "{command_name} failed: {tail}"
        ))),
    }
}

async fn drain_stderr(stderr: Option<tokio::process::ChildStderr>) -> Vec<u8> {
    match stderr {
        Some(stderr) => {
            drain_bounded(tokio::io::empty(), stderr, MAX_BYTES_PER_STREAM, || {})
                .await
                .stderr
        }
        None => Vec::new(),
    }
}

fn tail_string(bytes: &[u8], max: usize) -> String {
    let mut start = bytes.len().saturating_sub(max);
    // A cut inside a character would open the tail with a replacement mark.
    while bytes.get(start).is_some_and(|byte| byte & 0xC0 == 0x80) {
        start += 1;
    }
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn uv_builder(build: Build) -> BoxFuture<Result<(), EnvError>> {
    Box::pin(async move {
        let Some(uv) = crate::engines::uv::located() else {
            return Err(EnvError::RuntimeUnavailable(
                kernel::machine::UV_INSTALL_HINT.to_owned(),
            ));
        };
        uv_build(&uv.path, build).await
    })
}

/// What uv runs with beyond the environment every sidecar gets: the package
/// cache, where a Python it downloads goes, and `config_home`, the person's
/// `XDG_CONFIG_HOME` when it is an absolute path, so a mirror set in the
/// `uv.toml` there is found as it is under `HOME`.
fn uv_variables(
    cache_dir: &Path,
    python_dir: &Path,
    config_home: Option<&str>,
) -> BTreeMap<String, String> {
    let mut variables = BTreeMap::from([
        ("UV_CACHE_DIR".to_owned(), cache_dir.display().to_string()),
        (
            "UV_PYTHON_INSTALL_DIR".to_owned(),
            python_dir.display().to_string(),
        ),
    ]);
    if let Some(config_home) = config_home.filter(|path| Path::new(path).is_absolute()) {
        variables.insert("XDG_CONFIG_HOME".to_owned(), config_home.to_owned());
    }
    variables
}

/// Build the environment `build` describes with the `uv` at `uv`: create it
/// with Python 3.12, then install the lockfile exactly, from the cache alone
/// first and from the network when that is not enough. Every requirement must
/// match a hash in the lockfile, whatever a `uv.toml` says about hashes.
pub(crate) async fn uv_build(uv: &Path, build: Build) -> Result<(), EnvError> {
    let Build {
        env_dir,
        lockfile,
        cache_dir,
        python_dir,
        cpu_torch,
        progress,
    } = build;
    if cpu_torch {
        let path = uv.to_owned();
        let version = tokio::task::spawn_blocking(move || crate::engines::uv::version(&path))
            .await
            .ok()
            .flatten();
        if let Some(version) = version.filter(|version| !takes_torch_backend(version)) {
            let [major, minor, patch] = TORCH_BACKEND_SINCE;
            return Err(EnvError::RuntimeUnavailable(format!(
                "the uv at {} ({version}) cannot install PyTorch's CPU build, which needs uv {major}.{minor}.{patch} or later; run `hedos engines install uv`",
                uv.display()
            )));
        }
    }
    let uv_env = uv_variables(
        &cache_dir,
        &python_dir,
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
    );

    progress("Creating Python environment…");
    run_process(
        uv,
        &["venv", &env_dir.display().to_string(), "--python", "3.12"],
        &uv_env,
        DEFAULT_PROCESS_TIMEOUT,
    )
    .await?;

    let python = env_dir.join("bin/python").display().to_string();
    let lock = lockfile.display().to_string();
    let sync = |online: bool| {
        let mut args = vec![
            "pip",
            "sync",
            lock.as_str(),
            "--python",
            python.as_str(),
            "--require-hashes",
        ];
        // PyTorch's CPU builds are served by its own index only; this sends
        // torch and the packages around it (torchaudio, torchvision, ...)
        // there and every other package to uv's usual index, as the lock was
        // compiled.
        if cpu_torch {
            args.extend(["--torch-backend", "cpu"]);
        }
        if !online {
            args.push("--offline");
        }
        args
    };
    progress("Installing packages…");
    let offline = run_process(uv, &sync(false), &uv_env, DEFAULT_PROCESS_TIMEOUT).await;
    if offline.is_err() {
        progress("Downloading packages…");
        run_process(uv, &sync(true), &uv_env, DOWNLOAD_TIMEOUT).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `uv` at `dir/uv` that answers `--version` with `uv 0.13.4`, writes
    /// each other call's arguments and the two directories it was given to
    /// `dir/calls`, makes the environment on `venv`, fails any `--offline`
    /// sync when `offline_fails`, and deletes itself after `venv` when
    /// `vanishes`.
    fn recording_uv(dir: &Path, offline_fails: bool, vanishes: bool) -> PathBuf {
        uv_reporting(dir, "0.13.4", offline_fails, vanishes)
    }

    /// [`recording_uv`], reporting `version`.
    fn uv_reporting(dir: &Path, version: &str, offline_fails: bool, vanishes: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let calls = dir.join("calls");
        let uv = dir.join("uv");
        let script = format!(
            "#!/bin/sh\n\
             case \"$1\" in --version) echo \"uv {version} (aarch64-apple-darwin)\"; exit 0;; esac\n\
             echo \"$* | cache=$UV_CACHE_DIR | python=$UV_PYTHON_INSTALL_DIR\" >> '{calls}'\n\
             case \"$1\" in venv) mkdir -p \"$2/bin\"; {vanish} exit 0;; esac\n\
             case \"$*\" in *--offline*) [ {offline} = 1 ] && exit 1;; esac\n\
             exit 0\n",
            calls = calls.display(),
            vanish = if vanishes { "rm -f \"$0\";" } else { "" },
            offline = u8::from(offline_fails),
        );
        std::fs::write(&uv, script).unwrap();
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
        uv
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hedos-uv-build-{name}-{}-{}",
            std::process::id(),
            kernel::time::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manager_with(root: PathBuf, uv: PathBuf, cpu_only: fn() -> bool) -> EnvironmentManager {
        EnvironmentManager::seeing(
            root,
            Arc::new(move |build| {
                let uv = uv.clone();
                Box::pin(async move { uv_build(&uv, build).await })
            }),
            cpu_only,
        )
    }

    #[tokio::test]
    async fn a_build_insists_on_the_lock_s_hashes_and_keeps_python_in_hedos_s_data() {
        let dir = scratch("args");
        let uv = recording_uv(&dir, true, false);
        let lock = dir.join("requirements.lock");
        std::fs::write(&lock, "x==1 --hash=sha256:00").unwrap();
        let root = dir.join("env").join("mlx-lm");
        let env = manager_with(root.clone(), uv, || false)
            .prepare("python:test", &lock, Arc::new(|_: &str| {}))
            .await
            .expect("built");
        assert!(env.join(ENV_MARKER).exists());

        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        assert_eq!(calls.len(), 3, "{calls:?}");
        let cache = root.join("uv-cache").display().to_string();
        let python = dir.join("env").join("python").display().to_string();
        for call in &calls {
            assert!(
                call.ends_with(&format!("| cache={cache} | python={python}")),
                "{call}"
            );
        }
        assert!(calls[0].starts_with("venv "), "{}", calls[0]);
        assert!(calls[0].contains("--python 3.12"), "{}", calls[0]);
        for sync in &calls[1..] {
            assert!(sync.starts_with("pip sync "), "{sync}");
            assert!(sync.contains(" --require-hashes"), "{sync}");
            assert!(!sync.contains("--torch-backend"), "{sync}");
        }
        assert!(calls[1].contains("--offline"), "{}", calls[1]);
        assert!(!calls[2].contains("--offline"), "{}", calls[2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_uv_gone_before_the_sync_fails_the_build_and_leaves_it_to_be_built_again() {
        let dir = scratch("vanish");
        let uv = recording_uv(&dir, false, true);
        let lock = dir.join("requirements.lock");
        std::fs::write(&lock, "x==1").unwrap();
        let manager = manager_with(dir.join("env").join("mlx-lm"), uv.clone(), || false);
        let failed = manager
            .prepare("python:test", &lock, Arc::new(|_: &str| {}))
            .await;
        assert!(failed.is_err(), "{failed:?}");
        assert!(!uv.exists());
        let envs = dir.join("env/mlx-lm/runtimes/python-test/envs");
        let marked = std::fs::read_dir(&envs)
            .unwrap()
            .flatten()
            .any(|env| env.path().join(ENV_MARKER).exists());
        assert!(!marked);

        // With uv back, the next run builds it.
        let uv = recording_uv(&dir, false, false);
        let env = manager_with(dir.join("env").join("mlx-lm"), uv, || false)
            .prepare("python:test", &lock, Arc::new(|_: &str| {}))
            .await
            .expect("built");
        assert!(env.join(ENV_MARKER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `pip sync` calls `recording_uv` has seen, forgetting them.
    fn syncs(dir: &Path) -> Vec<String> {
        let calls = dir.join("calls");
        let seen = std::fs::read_to_string(&calls).unwrap_or_default();
        let _ = std::fs::remove_file(&calls);
        seen.lines()
            .filter(|call| call.starts_with("pip sync "))
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    async fn a_cpu_variant_is_installed_only_where_torch_runs_on_the_cpu() {
        let dir = scratch("variant");
        let uv = recording_uv(&dir, false, false);
        let lock = dir.join("requirements.lock");
        let variant = dir.join("requirements.cpu.lock");
        std::fs::write(&lock, "torch==1").unwrap();
        std::fs::write(&variant, "torch==1+cpu").unwrap();
        let root = dir.join("env").join("audio");
        let quiet: Progress = Arc::new(|_: &str| {});

        let cpu = manager_with(root.clone(), uv.clone(), || true);
        assert!(!cpu.is_built("python:audio", &lock));
        let on_cpu = cpu
            .prepare("python:audio", &lock, Arc::clone(&quiet))
            .await
            .unwrap();
        let synced = syncs(&dir);
        assert_eq!(synced.len(), 1, "{synced:?}");
        assert!(
            synced[0].starts_with(&format!("pip sync {} ", variant.display())),
            "{}",
            synced[0]
        );
        assert!(synced[0].contains(" --torch-backend cpu"), "{}", synced[0]);
        assert!(cpu.is_built("python:audio", &lock));

        let gpu = manager_with(root.clone(), uv, || false);
        assert!(!gpu.is_built("python:audio", &lock));
        let on_gpu = gpu
            .prepare("python:audio", &lock, Arc::clone(&quiet))
            .await
            .unwrap();
        let synced = syncs(&dir);
        assert_eq!(synced.len(), 1, "{synced:?}");
        assert!(
            synced[0].starts_with(&format!("pip sync {} ", lock.display())),
            "{}",
            synced[0]
        );
        assert!(!synced[0].contains("--torch-backend"), "{}", synced[0]);
        assert!(gpu.is_built("python:audio", &lock));
        assert_ne!(on_cpu, on_gpu);

        // Back on the CPU, the environment built first is still there and is
        // the one in use again, with nothing installed.
        let again = cpu
            .prepare("python:audio", &lock, Arc::clone(&quiet))
            .await
            .unwrap();
        assert_eq!(again, on_cpu);
        assert!(syncs(&dir).is_empty());
        assert!(on_gpu.join(ENV_MARKER).exists());
        let current = root.join("runtimes/python-audio/current");
        assert_eq!(std::fs::read_link(&current).unwrap(), on_cpu);

        let lone = dir.join("lone").join("requirements.lock");
        std::fs::create_dir_all(lone.parent().unwrap()).unwrap();
        std::fs::write(&lone, "x==1").unwrap();
        for manager in [&cpu, &gpu] {
            assert_eq!(
                manager.lock_here(&lone),
                Lock {
                    path: lone.clone(),
                    cpu_torch: false
                }
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_uv_too_old_for_the_cpu_build_is_named_with_the_way_to_a_newer_one() {
        let dir = scratch("old");
        let uv = uv_reporting(&dir, "0.6.8", false, false);
        let lock = dir.join("requirements.lock");
        std::fs::write(&lock, "torch==1").unwrap();
        std::fs::write(dir.join("requirements.cpu.lock"), "torch==1+cpu").unwrap();
        let quiet: Progress = Arc::new(|_: &str| {});

        let failed = manager_with(dir.join("env").join("audio"), uv.clone(), || true)
            .prepare("python:audio", &lock, Arc::clone(&quiet))
            .await;
        match failed {
            Err(EnvError::RuntimeUnavailable(message)) => {
                assert!(message.contains("(0.6.8)"), "{message}");
                assert!(message.contains("hedos engines install uv"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!dir.join("calls").exists());

        // The same uv still builds where nothing asks it for the CPU build.
        manager_with(dir.join("env").join("audio"), uv, || false)
            .prepare("python:audio", &lock, quiet)
            .await
            .unwrap();
        assert_eq!(syncs(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manager_asks_this_machine_whether_torch_runs_on_the_cpu() {
        let made = EnvironmentManager::new(std::env::temp_dir().join("hedos-env-machine"));
        assert!(std::ptr::fn_addr_eq(
            made.inner.cpu_only,
            crate::machine::torch_on_the_cpu as fn() -> bool
        ));
    }

    fn shipped_locks() -> Vec<PathBuf> {
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtimes");
        let mut locks: Vec<PathBuf> = std::fs::read_dir(&shipped)
            .unwrap()
            .flatten()
            .flat_map(|bundle| std::fs::read_dir(bundle.path()).into_iter().flatten())
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "lock")
            })
            .collect();
        locks.sort();
        locks
    }

    /// The requirement lines of a lock, each with its continuation lines.
    fn requirements(text: &str) -> Vec<String> {
        text.replace("\\\n", " ")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('-'))
            .map(str::to_owned)
            .collect()
    }

    /// A runtime whose lock pins torch for Linux (not an MLX one) installs
    /// PyPI's torch from `requirements.lock`, the CUDA build with NVIDIA's
    /// libraries on Linux, and ships `requirements.cpu.lock` with PyTorch's
    /// CPU build for a Linux machine without NVIDIA's driver.
    #[test]
    fn a_shipped_torch_lock_has_a_cpu_variant_and_cuda_on_linux() {
        let mut checked = 0;
        for lockfile in shipped_locks() {
            if lockfile.file_name().and_then(|name| name.to_str()) != Some("requirements.lock") {
                continue;
            }
            let text = std::fs::read_to_string(&lockfile).unwrap();
            let pins = requirements(&text);
            let pinned = |pins: &[String], prefix: &str| -> Vec<String> {
                pins.iter()
                    .filter(|line| line.starts_with(prefix))
                    .cloned()
                    .collect()
            };
            if pinned(&pins, "torch==").is_empty() || !pinned(&pins, "mlx==").is_empty() {
                continue;
            }
            checked += 1;
            let shown = lockfile.display();
            assert!(
                !text.lines().any(|line| line.starts_with("--")),
                "{shown}: hedos chooses the index, not the lock"
            );
            let torch = pinned(&pins, "torch==");
            assert!(
                torch.len() == 1 && !torch[0].contains("+cpu") && !torch[0].contains(" ; "),
                "{shown}: {torch:?}"
            );
            let cuda_on = |machine: &str| {
                pins.iter().any(|line| {
                    line.starts_with("nvidia-cublas")
                        && line.contains("sys_platform == 'linux'")
                        && (!line.contains("platform_machine") || line.contains(machine))
                })
            };
            assert!(cuda_on("x86_64"), "{shown}");
            // PyPI's aarch64 torch carries CUDA from 2.11 on (YuE2 pins 2.10,
            // so it runs on the CPU on an ARM machine with NVIDIA cards).
            let version: Vec<u32> = torch[0]
                .trim_start_matches("torch==")
                .split(|c: char| !c.is_ascii_digit())
                .take(2)
                .filter_map(|part| part.parse().ok())
                .collect();
            if version >= vec![2, 11] {
                assert!(cuda_on("aarch64"), "{shown}");
            }

            let variant = lockfile.with_file_name("requirements.cpu.lock");
            let shown = variant.display();
            let text = std::fs::read_to_string(&variant)
                .unwrap_or_else(|error| panic!("{shown}: {error}"));
            let pins = requirements(&text);
            assert!(
                text.lines().any(|line| line.starts_with('#')
                    && line.contains(" --torch-backend cpu ")
                    && line.ends_with(" -o requirements.cpu.lock")),
                "{shown}"
            );
            assert!(
                !text.lines().any(|line| line.starts_with("--")),
                "{shown}: hedos chooses the index, not the lock"
            );
            let cpu = pinned(&pins, "torch==");
            assert!(
                cpu.iter()
                    .any(|line| line.contains("+cpu ; sys_platform != 'darwin'")),
                "{shown}: {cpu:?}"
            );
            assert!(pinned(&pins, "nvidia-").is_empty(), "{shown}");
        }
        assert!(checked >= 5, "{checked} torch locks");
    }

    /// Builds sync with `--require-hashes`, so a shipped lock with a
    /// requirement carrying no hash would fail every build of its runtime.
    #[test]
    fn every_requirement_in_a_shipped_lock_carries_a_hash() {
        let mut locks = 0;
        for lockfile in shipped_locks() {
            let Ok(text) = std::fs::read_to_string(&lockfile) else {
                continue;
            };
            locks += 1;
            let joined = text.replace("\\\n", " ");
            for line in joined.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
                    continue;
                }
                assert!(
                    line.contains("--hash=sha256:"),
                    "{}: {line}",
                    lockfile.display()
                );
            }
        }
        assert!(locks >= 12, "{locks} locks");
    }

    /// How a shipped lockfile is checked against the pinned uv, on a machine
    /// with no Python 3.12 of its own: the environment builds from it with
    /// every hash required, and the Python uv fetches lands under the
    /// environments directory, not uv's own.
    #[tokio::test]
    #[ignore = "needs HEDOS_REAL_UV, a uv to build with, and the network"]
    async fn a_shipped_lock_builds_with_a_real_uv() {
        let uv = PathBuf::from(std::env::var("HEDOS_REAL_UV").expect("HEDOS_REAL_UV"));
        let lockfile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtimes/python-whisper-cpp/requirements.lock");
        let dir = scratch("real");
        let root = dir.join("env").join("whisper-cpp");
        let env = manager_with(root, uv, || false)
            .prepare("python:whisper-cpp", &lockfile, Arc::new(|_: &str| {}))
            .await
            .expect("built");
        assert!(env.join(ENV_MARKER).exists());
        let config = std::fs::read_to_string(env.join("pyvenv.cfg")).unwrap();
        let home = config
            .lines()
            .find_map(|line| line.strip_prefix("home = "))
            .expect("the Python it was made with");
        assert!(
            Path::new(home).starts_with(dir.join("env").join("python")),
            "{home}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uv_finds_a_config_under_xdg_config_home_only_when_that_is_absolute() {
        let (cache, python) = (Path::new("/env/x/uv-cache"), Path::new("/env/python"));
        let with = uv_variables(cache, python, Some("/home/you/.cfg"));
        assert_eq!(
            with.get("XDG_CONFIG_HOME").map(String::as_str),
            Some("/home/you/.cfg")
        );
        assert_eq!(
            with.get("UV_CACHE_DIR").map(String::as_str),
            Some("/env/x/uv-cache")
        );
        assert_eq!(
            with.get("UV_PYTHON_INSTALL_DIR").map(String::as_str),
            Some("/env/python")
        );
        assert!(!uv_variables(cache, python, None).contains_key("XDG_CONFIG_HOME"));
        assert!(!uv_variables(cache, python, Some("cfg")).contains_key("XDG_CONFIG_HOME"));
    }

    #[test]
    fn every_manager_under_one_environments_directory_shares_one_python() {
        let env = Path::new("/data/env");
        let built_in = EnvironmentManager::new(env.join("mlx-lm"));
        let manifests = EnvironmentManager::new(env.join("manifests"));
        assert_eq!(built_in.python_dir(), env.join("python"));
        assert_eq!(manifests.python_dir(), env.join("python"));
    }

    #[test]
    fn a_tail_never_opens_inside_a_character() {
        assert_eq!(tail_string("aé".as_bytes(), 1), "");
        assert_eq!(tail_string("aé".as_bytes(), 2), "é");
        assert_eq!(tail_string("abc".as_bytes(), 2), "bc");
        assert_eq!(tail_string("abc".as_bytes(), 0), "");
    }

    fn base(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_baseline_key_passes_through() {
        let env = scrubbed_environment(base(&[("PATH", "/usr/bin")]), &BTreeMap::new());
        assert_eq!(env.get("PATH"), Some(&"/usr/bin".to_owned()));
    }

    #[test]
    fn a_secret_shaped_key_is_dropped() {
        let env = scrubbed_environment(
            base(&[
                ("AWS_SECRET_ACCESS_KEY", "secret"),
                ("GITHUB_TOKEN", "token"),
            ]),
            &BTreeMap::new(),
        );
        assert!(!env.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!env.contains_key("GITHUB_TOKEN"));
    }

    #[test]
    fn an_override_wins_even_outside_the_baseline() {
        let overrides = BTreeMap::from([("HF_HOME".to_owned(), "/custom/hf".to_owned())]);
        let env = scrubbed_environment(base(&[("HF_HOME", "/parent/hf")]), &overrides);
        assert_eq!(env.get("HF_HOME"), Some(&"/custom/hf".to_owned()));
    }

    #[test]
    fn pythonpath_and_pythonhome_are_still_absent() {
        let env = scrubbed_environment(
            base(&[("PYTHONPATH", "/evil"), ("PYTHONHOME", "/evil")]),
            &BTreeMap::new(),
        );
        assert!(!env.contains_key("PYTHONPATH"));
        assert!(!env.contains_key("PYTHONHOME"));
    }
}
