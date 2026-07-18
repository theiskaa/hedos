//! Tests for `EnvironmentManager`: lock-hash keying, the build/relink/marker
//! lifecycle with an injected builder, environment scrubbing, and the
//! honest-failure `run_process` wrapper (real `/bin/sh` children).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use runtime::environment::{
    Builder, EnvironmentManager, Progress, run_process, scrubbed_environment,
};
use runtime::governor::BoxFuture;
use support::TempDir;

fn noop_progress() -> Progress {
    Arc::new(|_: &str| {})
}

fn making_builder() -> Builder {
    Arc::new(
        |env_dir: PathBuf, _lock: PathBuf, _cache: PathBuf, _p: Progress| -> BoxFuture<_> {
            Box::pin(async move {
                std::fs::create_dir_all(&env_dir)?;
                Ok(())
            })
        },
    )
}

fn write_lock(dir: &TempDir, contents: &str) -> PathBuf {
    let path = dir.join("uv.lock");
    std::fs::write(&path, contents).expect("write lock");
    path
}

#[test]
fn lock_hash_is_stable_and_content_addressed() {
    let dir = TempDir::new();
    let a = write_lock(&dir, "deps-a");
    let b = dir.join("other.lock");
    std::fs::write(&b, "deps-a").expect("write");
    let c = dir.join("c.lock");
    std::fs::write(&c, "deps-b").expect("write");

    let ha = EnvironmentManager::lock_hash(&a).expect("hash");
    assert_eq!(ha.len(), 16, "8 bytes hex-encoded");
    assert_eq!(
        ha,
        EnvironmentManager::lock_hash(&b).expect("hash"),
        "same content, same hash"
    );
    assert_ne!(
        ha,
        EnvironmentManager::lock_hash(&c).expect("hash"),
        "different content, different hash"
    );
}

#[tokio::test]
async fn prepare_builds_once_then_relinks_on_repeat() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps");
    let builds = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&builds);
    let builder: Builder = Arc::new(move |env_dir: PathBuf, _l, _c, _p| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::Relaxed);
            std::fs::create_dir_all(&env_dir)?;
            Ok(())
        })
    });
    let manager = EnvironmentManager::with_builder(dir.path().to_owned(), builder);

    let env = manager
        .prepare("python:test", &lock, noop_progress())
        .await
        .expect("prepare");
    assert!(env.exists());
    assert!(
        env.join(".hedos-env-ok").exists(),
        "the ok marker is written"
    );

    let again = manager
        .prepare("python:test", &lock, noop_progress())
        .await
        .expect("prepare");
    assert_eq!(env, again);
    assert_eq!(
        builds.load(Ordering::Relaxed),
        1,
        "the marker short-circuits the rebuild"
    );

    let current = dir.path().join("runtimes/python-test/current");
    assert_eq!(
        std::fs::read_link(&current).expect("current is a symlink"),
        env,
        "current points at the active env"
    );
}

#[tokio::test]
async fn a_changed_lockfile_rebuilds_into_a_new_env() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps-v1");
    let manager = EnvironmentManager::with_builder(dir.path().to_owned(), making_builder());

    let first = manager
        .prepare("r", &lock, noop_progress())
        .await
        .expect("prepare");
    std::fs::write(&lock, "deps-v2").expect("rewrite lock");
    let second = manager
        .prepare("r", &lock, noop_progress())
        .await
        .expect("prepare");

    assert_ne!(first, second, "a new lock hash is a new env directory");
    assert!(first.exists() && second.exists());
}

#[tokio::test]
async fn a_failed_build_leaves_no_marker_and_retries() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps");
    let attempts = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&attempts);
    let builder: Builder = Arc::new(move |env_dir: PathBuf, _l, _c, _p| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            if counter.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(runtime::environment::EnvError::RuntimeFailed(
                    "first attempt dies".to_owned(),
                ));
            }
            std::fs::create_dir_all(&env_dir)?;
            Ok(())
        })
    });
    let manager = EnvironmentManager::with_builder(dir.path().to_owned(), builder);

    assert!(
        manager
            .prepare("python:test", &lock, noop_progress())
            .await
            .is_err(),
        "the first build fails"
    );
    let env = manager
        .prepare("python:test", &lock, noop_progress())
        .await
        .expect("retry succeeds");
    assert!(env.exists());
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        2,
        "the failure did not stick"
    );
}

#[test]
fn scrubbed_environment_strips_python_vars_and_applies_overrides() {
    let base = [
        ("PATH".to_owned(), "/bin".to_owned()),
        ("PYTHONPATH".to_owned(), "/danger".to_owned()),
        ("PYTHONHOME".to_owned(), "/danger".to_owned()),
        ("KEEP".to_owned(), "yes".to_owned()),
    ];
    let overrides = BTreeMap::from([("EXTRA".to_owned(), "1".to_owned())]);
    let env = scrubbed_environment(base, &overrides);

    assert!(!env.contains_key("PYTHONPATH"));
    assert!(!env.contains_key("PYTHONHOME"));
    assert_eq!(env.get("KEEP").map(String::as_str), Some("yes"));
    assert_eq!(env.get("EXTRA").map(String::as_str), Some("1"));
}

#[tokio::test]
async fn run_process_drains_a_flood_of_stderr_without_deadlocking() {
    run_process(
        Path::new("/bin/sh"),
        &[
            "-c",
            "dd if=/dev/zero bs=1024 count=256 2>/dev/null | tr '\\0' x >&2",
        ],
        &BTreeMap::new(),
        Duration::from_secs(30),
    )
    .await
    .expect("a 256 KiB stderr flood still completes");
}

#[tokio::test]
async fn run_process_times_out_a_hung_child_honestly() {
    let err = run_process(
        Path::new("/bin/sleep"),
        &["60"],
        &BTreeMap::new(),
        Duration::from_secs(1),
    )
    .await
    .expect_err("a hung child must fail");
    let message = err.to_string();
    assert!(message.contains("timed out after 1s"), "got: {message}");
    assert!(message.contains("sleep"), "names the command: {message}");
}

#[tokio::test]
async fn run_process_reports_a_nonzero_exit() {
    let err = run_process(
        Path::new("/bin/sh"),
        &["-c", "echo boom >&2; exit 3"],
        &BTreeMap::new(),
        Duration::from_secs(10),
    )
    .await
    .expect_err("a failing child must fail");
    let message = err.to_string();
    assert!(message.contains("failed"), "got: {message}");
    assert!(
        message.contains("boom"),
        "carries the stderr tail: {message}"
    );
}

fn counting_slow_builder(counter: Arc<AtomicU32>) -> Builder {
    Arc::new(move |env_dir: PathBuf, _l, _c, _p| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(200)).await;
            std::fs::create_dir_all(&env_dir)?;
            Ok(())
        })
    })
}

#[tokio::test]
async fn concurrent_prepares_for_one_runtime_coalesce_into_a_single_build() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps");
    let builds = Arc::new(AtomicU32::new(0));
    let manager = EnvironmentManager::with_builder(
        dir.path().to_owned(),
        counting_slow_builder(Arc::clone(&builds)),
    );

    let first = manager.prepare("python:test", &lock, noop_progress());
    let second = manager.prepare("python:test", &lock, noop_progress());
    let (first, second) = tokio::join!(first, second);
    let first = first.expect("first");
    let second = second.expect("second");

    assert_eq!(first, second);
    assert_eq!(
        builds.load(Ordering::Relaxed),
        1,
        "the second prepare relinks, not rebuilds"
    );
    assert!(first.join(".hedos-env-ok").exists());
}

#[tokio::test]
async fn prepares_for_different_runtimes_run_independently() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps");
    let builds = Arc::new(AtomicU32::new(0));
    let manager = EnvironmentManager::with_builder(
        dir.path().to_owned(),
        counting_slow_builder(Arc::clone(&builds)),
    );

    let one = manager.prepare("python:one", &lock, noop_progress());
    let two = manager.prepare("python:two", &lock, noop_progress());
    let _ = tokio::join!(one, two);

    assert_eq!(
        builds.load(Ordering::Relaxed),
        2,
        "distinct runtimes each build"
    );
}

#[tokio::test]
async fn rolling_back_to_a_prior_lockfile_relinks_without_rebuilding() {
    let dir = TempDir::new();
    let lock = write_lock(&dir, "deps-a");
    let builds = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&builds);
    let builder: Builder = Arc::new(move |env_dir: PathBuf, _l, _c, _p| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::Relaxed);
            std::fs::create_dir_all(&env_dir)?;
            Ok(())
        })
    });
    let manager = EnvironmentManager::with_builder(dir.path().to_owned(), builder);

    let first = manager
        .prepare("r", &lock, noop_progress())
        .await
        .expect("build a");
    std::fs::write(&lock, "deps-b").expect("rewrite");
    manager
        .prepare("r", &lock, noop_progress())
        .await
        .expect("build b");
    std::fs::write(&lock, "deps-a").expect("restore");
    let rolled_back = manager
        .prepare("r", &lock, noop_progress())
        .await
        .expect("relink a");

    assert_eq!(rolled_back, first, "rolling back reuses the first env");
    assert_eq!(
        builds.load(Ordering::Relaxed),
        2,
        "only two distinct locks were ever built"
    );
}
