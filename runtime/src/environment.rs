//! Python environment provisioning: one virtual environment per runtime, keyed
//! by the SHA-256 of its lockfile so a changed lock rebuilds and an unchanged
//! one short-circuits to a relink. The build itself is injectable (the default
//! shells out to the `uv` [`crate::engines::uv::located`] finds), and
//! honest-failure process running lives here too.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Where each step is reported.
    pub progress: Progress,
}

/// Builds the environment [`Build`] describes.
pub type Builder = Arc<dyn Fn(Build) -> BoxFuture<Result<(), EnvError>> + Send + Sync>;

const ENV_MARKER: &str = ".hedos-env-ok";
const STDERR_TAIL_BYTES: usize = 400;
const DEFAULT_PROCESS_TIMEOUT: Duration = Duration::from_secs(900);

struct Inner {
    root: PathBuf,
    builder: Builder,
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
    pub fn new(root: PathBuf) -> Self {
        Self::with_builder(root, Arc::new(uv_builder))
    }

    /// A manager rooted at `root` that builds environments with `builder`
    /// (injected for tests).
    pub fn with_builder(root: PathBuf, builder: Builder) -> Self {
        Self {
            inner: Arc::new(Inner {
                root,
                builder,
                key_locks: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Whether the environment for `runtime_id` at the current `lockfile` is
    /// built: [`prepare`](Self::prepare) would return it without building.
    pub(crate) fn is_built(&self, runtime_id: &str, lockfile: &Path) -> bool {
        Self::lock_hash(lockfile).is_ok_and(|hash| {
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

    /// Ensure the environment for `runtime_id` at the current `lockfile` exists,
    /// building it if missing, and return its directory. Concurrent prepares for
    /// the same runtime+lock serialize; a build runs at most once (a later caller
    /// sees the marker and only relinks). A failed build leaves no marker, so it
    /// can be retried.
    pub async fn prepare(
        &self,
        runtime_id: &str,
        lockfile: &Path,
        progress: Progress,
    ) -> Result<PathBuf, EnvError> {
        let hash = Self::lock_hash(lockfile)?;
        let key = format!("{runtime_id}#{hash}");
        let key_lock = {
            let mut locks = lock(&self.inner.key_locks);
            Arc::clone(locks.entry(key).or_default())
        };
        let _guard = key_lock.lock().await;
        self.perform_prepare(runtime_id, &hash, lockfile, progress)
            .await
    }

    async fn perform_prepare(
        &self,
        runtime_id: &str,
        hash: &str,
        lockfile: &Path,
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
            lockfile: lockfile.to_owned(),
            cache_dir,
            python_dir: self.python_dir(),
            progress: Arc::clone(&progress),
        })
        .await?;

        std::fs::write(&marker, [])?;
        relink(&current, &env_dir)?;
        Ok(env_dir)
    }
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
        progress,
    } = build;
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
        if !online {
            args.push("--offline");
        }
        args
    };
    progress("Installing packages…");
    let offline = run_process(uv, &sync(false), &uv_env, DEFAULT_PROCESS_TIMEOUT).await;
    if offline.is_err() {
        progress("Downloading packages…");
        run_process(uv, &sync(true), &uv_env, DEFAULT_PROCESS_TIMEOUT).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `uv` at `dir/uv` that writes each call's arguments and the two
    /// directories it was given to `dir/calls`, makes the environment on
    /// `venv`, fails any `--offline` sync when `offline_fails`, and deletes
    /// itself after `venv` when `vanishes`.
    fn recording_uv(dir: &Path, offline_fails: bool, vanishes: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let calls = dir.join("calls");
        let uv = dir.join("uv");
        let script = format!(
            "#!/bin/sh\n\
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

    fn manager_with(root: PathBuf, uv: PathBuf) -> EnvironmentManager {
        EnvironmentManager::with_builder(
            root,
            Arc::new(move |build| {
                let uv = uv.clone();
                Box::pin(async move { uv_build(&uv, build).await })
            }),
        )
    }

    #[tokio::test]
    async fn a_build_insists_on_the_lock_s_hashes_and_keeps_python_in_hedos_s_data() {
        let dir = scratch("args");
        let uv = recording_uv(&dir, true, false);
        let lock = dir.join("requirements.lock");
        std::fs::write(&lock, "x==1 --hash=sha256:00").unwrap();
        let root = dir.join("env").join("mlx-lm");
        let env = manager_with(root.clone(), uv)
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
        let manager = manager_with(dir.join("env").join("mlx-lm"), uv.clone());
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
        let env = manager_with(dir.join("env").join("mlx-lm"), uv)
            .prepare("python:test", &lock, Arc::new(|_: &str| {}))
            .await
            .expect("built");
        assert!(env.join(ENV_MARKER).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Builds sync with `--require-hashes`, so a shipped lock with a
    /// requirement carrying no hash would fail every build of its runtime.
    #[test]
    fn every_requirement_in_a_shipped_lock_carries_a_hash() {
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtimes");
        let mut locks = 0;
        for bundle in std::fs::read_dir(&shipped).unwrap().flatten() {
            let lockfile = bundle.path().join("requirements.lock");
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
        assert!(locks >= 7, "{locks} locks under {}", shipped.display());
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
        let env = manager_with(root, uv)
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
