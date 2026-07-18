//! Host-side process containment: enumerate a process tree, terminate it
//! gracefully then forcefully, and drain a child's stdout/stderr into bounded
//! buffers. The sidecar supervisor spawns and reaps subprocesses through here.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;

/// Default per-stream byte cap for a [`drain_bounded`] collection.
pub const MAX_BYTES_PER_STREAM: usize = 16 * 1024 * 1024;

const SIGTERM: i32 = 15;
const SIGKILL: i32 = 9;

/// Whether a process with `pid` currently exists. Uses a signal-0 probe, which
/// asks the kernel about deliverability without sending a signal.
pub fn is_running(pid: i32) -> bool {
    signal(pid, 0)
}

/// The descendant pids of `root`, given a table of `(pid, parent_pid)` pairs.
/// Breadth-first, so children precede grandchildren; a pid reachable through a
/// cycle (which the OS should never produce) is visited at most once.
pub fn descendants_of(pairs: &[(i32, i32)], root: i32) -> Vec<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for &(pid, ppid) in pairs {
        children.entry(ppid).or_default().push(pid);
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut frontier = VecDeque::from([root]);
    while let Some(current) = frontier.pop_front() {
        for &child in children.get(&current).into_iter().flatten() {
            if seen.insert(child) {
                result.push(child);
                frontier.push_back(child);
            }
        }
    }
    result
}

/// The live descendant pids of `pid`, discovered by parsing `ps`. Returns empty
/// if `ps` cannot be run or its output cannot be read.
pub async fn descendant_pids(pid: i32) -> Vec<i32> {
    let output = tokio::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .await;
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    descendants_of(&parse_ps(&text), pid)
}

fn parse_ps(text: &str) -> Vec<(i32, i32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            if fields.next().is_some() {
                return None;
            }
            Some((pid, ppid))
        })
        .collect()
}

/// Terminate the process tree rooted at `child`: `SIGTERM` the root and every
/// descendant, wait `grace`, then `SIGKILL` any survivor (twice, to catch a
/// process that spawned children only after the first sweep). The root's kill is
/// gated on `child`'s own liveness through `try_wait`, so — unlike the pid-based
/// descendant sweep — it can never signal an unrelated process that reused the
/// pid. A `child` that has already exited is a no-op.
///
/// Awaiting this costs at least `grace`, so do not hold a lock across it. The
/// caller keeps ownership of `child` and may `wait` it afterward to reap it.
pub async fn terminate_tree(child: &mut Child, grace: Duration) {
    let Some(pid) = child.id() else {
        return;
    };
    let pid = pid as i32;
    let descendants = descendant_pids(pid).await;
    signal(pid, SIGTERM);
    for &descendant in &descendants {
        signal(descendant, SIGTERM);
    }
    tokio::time::sleep(grace).await;
    for _ in 0..2 {
        for descendant in descendant_pids(pid).await {
            if is_running(descendant) {
                signal(descendant, SIGKILL);
            }
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.start_kill();
        }
    }
}

/// What a [`drain_bounded`] collection gathered. `exceeded` is set if either
/// stream produced more than the cap; the buffers hold at most the cap plus one
/// final chunk of bytes each.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DrainOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exceeded: bool,
}

/// Read `stdout` and `stderr` concurrently to EOF, buffering up to `max_bytes`
/// per stream. Bytes past the cap are read and discarded (so the child never
/// blocks on a full pipe) and `on_cap_exceeded` fires once, the first time
/// either stream crosses the cap. Dropping the returned future stops draining.
pub async fn drain_bounded<O, E, F>(
    mut stdout: O,
    mut stderr: E,
    max_bytes: usize,
    on_cap_exceeded: F,
) -> DrainOutcome
where
    O: AsyncRead + Unpin,
    E: AsyncRead + Unpin,
    F: Fn(),
{
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut out_exceeded = false;
    let mut err_exceeded = false;
    let mut out_done = false;
    let mut err_done = false;
    let mut fired = false;
    let mut out_buf = vec![0u8; 64 * 1024];
    let mut err_buf = vec![0u8; 64 * 1024];

    while !out_done || !err_done {
        tokio::select! {
            read = stdout.read(&mut out_buf), if !out_done => match read {
                Ok(0) | Err(_) => out_done = true,
                Ok(n) => append(&mut out, &out_buf[..n], max_bytes, &mut out_exceeded),
            },
            read = stderr.read(&mut err_buf), if !err_done => match read {
                Ok(0) | Err(_) => err_done = true,
                Ok(n) => append(&mut err, &err_buf[..n], max_bytes, &mut err_exceeded),
            },
        }
        if (out_exceeded || err_exceeded) && !fired {
            fired = true;
            on_cap_exceeded();
        }
    }

    DrainOutcome {
        stdout: out,
        stderr: err,
        exceeded: out_exceeded || err_exceeded,
    }
}

fn append(buf: &mut Vec<u8>, data: &[u8], max_bytes: usize, exceeded: &mut bool) {
    if *exceeded {
        return;
    }
    buf.extend_from_slice(data);
    if buf.len() > max_bytes {
        *exceeded = true;
    }
}

#[cfg(unix)]
fn signal(pid: i32, sig: i32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    // SAFETY: `kill` is a POSIX syscall wrapper with no memory effects — it
    // takes a pid and signal number by value and reports delivery/reachability
    // through its return value, so any argument pair is sound to pass.
    unsafe { kill(pid, sig) == 0 }
}

#[cfg(not(unix))]
fn signal(_pid: i32, _sig: i32) -> bool {
    false
}
