//! Python environment provisioning: one virtual environment per runtime, keyed
//! by the SHA-256 of its lockfile so a changed lock rebuilds and an unchanged
//! one short-circuits to a relink. The build itself is injectable (the default
//! shells out to `uv`), and honest-failure process running lives here too.

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

/// Builds the environment at `env_dir` from `lockfile`, using `cache_dir` as a
/// shared package cache and reporting steps through `progress`.
pub type Builder = Arc<
    dyn Fn(PathBuf, PathBuf, PathBuf, Progress) -> BoxFuture<Result<(), EnvError>> + Send + Sync,
>;

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

        (self.inner.builder)(
            env_dir.clone(),
            lockfile.to_owned(),
            cache_dir,
            Arc::clone(&progress),
        )
        .await?;

        std::fs::write(&marker, [])?;
        relink(&current, &env_dir)?;
        Ok(env_dir)
    }
}

/// The child environment for a sidecar or build: the current process
/// environment with `PYTHONPATH`/`PYTHONHOME` stripped (so a user's shell Python
/// config can't leak into the venv), then `overrides` applied.
pub fn scrubbed_environment<I>(
    base: I,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env: BTreeMap<String, String> = base.into_iter().collect();
    env.remove("PYTHONPATH");
    env.remove("PYTHONHOME");
    for (key, value) in overrides {
        env.insert(key.clone(), value.clone());
    }
    env
}

fn sanitized_runtime_id(id: &str) -> String {
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
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn uv_binary() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates = vec![
        format!("{home}/.local/bin/uv"),
        "/opt/homebrew/bin/uv".to_owned(),
        "/usr/local/bin/uv".to_owned(),
    ];
    if let Ok(path) = std::env::var("PATH") {
        candidates.extend(path.split(':').map(|entry| format!("{entry}/uv")));
    }
    candidates
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn uv_builder(
    env_dir: PathBuf,
    lockfile: PathBuf,
    cache_dir: PathBuf,
    progress: Progress,
) -> BoxFuture<Result<(), EnvError>> {
    Box::pin(async move {
        let Some(uv) = uv_binary() else {
            return Err(EnvError::RuntimeUnavailable(
                "uv is required to prepare Python runtimes. Install it from astral.sh/uv."
                    .to_owned(),
            ));
        };
        let cache = BTreeMap::from([("UV_CACHE_DIR".to_owned(), cache_dir.display().to_string())]);

        progress("Creating Python environment…");
        run_process(
            &uv,
            &["venv", &env_dir.display().to_string(), "--python", "3.12"],
            &cache,
            DEFAULT_PROCESS_TIMEOUT,
        )
        .await?;

        let python = env_dir.join("bin/python").display().to_string();
        let lock = lockfile.display().to_string();
        progress("Installing packages…");
        let offline = run_process(
            &uv,
            &["pip", "sync", &lock, "--python", &python, "--offline"],
            &cache,
            DEFAULT_PROCESS_TIMEOUT,
        )
        .await;
        if offline.is_err() {
            progress("Downloading packages…");
            run_process(
                &uv,
                &["pip", "sync", &lock, "--python", &python],
                &cache,
                DEFAULT_PROCESS_TIMEOUT,
            )
            .await?;
        }
        Ok(())
    })
}
