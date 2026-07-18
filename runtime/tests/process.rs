//! Tests for host-side process containment: the pure descendant-graph walk and
//! `ps` parsing (deterministic, no processes), bounded pipe draining (in-memory
//! and real pipes), and real spawn + tree-kill smoke tests (unix only).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use runtime::process::{DrainOutcome, MAX_BYTES_PER_STREAM, descendants_of, drain_bounded};

#[test]
fn descendants_walks_the_tree_breadth_first() {
    let pairs = [(2, 1), (3, 1), (4, 2), (5, 4), (6, 99)];
    assert_eq!(descendants_of(&pairs, 1), vec![2, 3, 4, 5]);
    assert_eq!(descendants_of(&pairs, 2), vec![4, 5]);
    assert_eq!(descendants_of(&pairs, 6), Vec::<i32>::new());
}

#[test]
fn descendants_of_a_leaf_or_unknown_pid_is_empty() {
    let pairs = [(2, 1), (3, 2)];
    assert!(descendants_of(&pairs, 3).is_empty());
    assert!(descendants_of(&pairs, 404).is_empty());
}

#[test]
fn descendants_tolerates_a_cycle_without_looping_forever() {
    let pairs = [(1, 2), (2, 1)];
    let mut got = descendants_of(&pairs, 1);
    got.sort_unstable();
    assert_eq!(got, vec![1, 2]);
}

#[tokio::test]
async fn drain_collects_both_streams_to_eof() {
    let outcome = drain_bounded(
        &b"hello stdout"[..],
        &b"hello stderr"[..],
        MAX_BYTES_PER_STREAM,
        || {},
    )
    .await;
    assert_eq!(
        outcome,
        DrainOutcome {
            stdout: b"hello stdout".to_vec(),
            stderr: b"hello stderr".to_vec(),
            exceeded: false,
        }
    );
}

#[tokio::test]
async fn drain_caps_the_buffer_and_signals_once() {
    let big = vec![b'x'; 1000];
    let calls = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&calls);
    let outcome = drain_bounded(&big[..], &b""[..], 100, move || {
        counter.fetch_add(1, Ordering::Relaxed);
    })
    .await;

    assert!(outcome.exceeded);
    assert!(
        outcome.stdout.len() <= 100 + 64 * 1024,
        "buffering stops near the cap"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "the cap callback fires exactly once"
    );
}

#[tokio::test]
async fn drain_fires_the_cap_callback_once_even_when_both_streams_overflow() {
    let big = vec![b'x'; 1000];
    let calls = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&calls);
    let outcome = drain_bounded(&big[..], &big[..], 100, move || {
        counter.fetch_add(1, Ordering::Relaxed);
    })
    .await;

    assert!(outcome.exceeded);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "both streams overflowing still fires the callback exactly once"
    );
}

#[tokio::test]
async fn drain_of_empty_streams_is_empty() {
    let outcome = drain_bounded(&b""[..], &b""[..], MAX_BYTES_PER_STREAM, || {}).await;
    assert_eq!(outcome, DrainOutcome::default());
}

#[tokio::test(start_paused = true)]
async fn drain_does_not_return_until_both_streams_reach_eof() {
    // A duplex whose write half is held open never reaches EOF, so the drain
    // must keep waiting — dropping it (here via `timeout`) is the only way out.
    let (_writer, reader) = tokio::io::duplex(64);
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        drain_bounded(reader, &b""[..], MAX_BYTES_PER_STREAM, || {}),
    )
    .await;
    assert!(
        result.is_err(),
        "an unfinished stream keeps the drain alive rather than returning early"
    );
}

#[cfg(unix)]
mod real_processes {
    use super::*;
    use runtime::process::{descendant_pids, is_running, terminate_tree};

    fn spawn_sleeper() -> tokio::process::Child {
        tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep")
    }

    async fn wait_pid_gone(pid: i32) -> bool {
        for _ in 0..100 {
            if !is_running(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn terminate_tree_kills_a_child_promptly() {
        let mut child = spawn_sleeper();
        let pid = child.id().expect("pid") as i32;
        terminate_tree(&mut child, Duration::from_millis(200)).await;
        let _ = child.wait().await;
        assert!(wait_pid_gone(pid).await, "the child is gone");
    }

    #[tokio::test]
    async fn terminate_tree_kills_a_shells_children() {
        let mut parent = tokio::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30 & sleep 30"])
            .spawn()
            .expect("spawn shell");
        let pid = parent.id().expect("pid") as i32;
        tokio::time::sleep(Duration::from_millis(200)).await;

        let kids = descendant_pids(pid).await;
        assert!(
            kids.len() >= 2,
            "the shell's two sleeps are descendants, got {kids:?}"
        );

        terminate_tree(&mut parent, Duration::from_millis(200)).await;
        let _ = parent.wait().await;
        for kid in kids {
            assert!(wait_pid_gone(kid).await, "descendant {kid} is dead");
        }
    }

    #[tokio::test]
    async fn terminate_tree_on_an_exited_child_is_a_noop() {
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn shell");
        let _ = child.wait().await;
        terminate_tree(&mut child, Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn draining_past_the_cap_lets_a_real_child_finish() {
        // The child writes far more than a pipe buffer holds; only because the
        // drain keeps reading and discarding past the cap can it ever exit.
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "head -c 400000 /dev/zero"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn writer");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let outcome = drain_bounded(stdout, stderr, 1000, || {}).await;
        assert!(outcome.exceeded, "the 400 KB write blows the 1 KB cap");

        let status = child.wait().await.expect("wait");
        assert!(status.success(), "the child completed rather than blocking");
    }
}
