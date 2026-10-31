//! The llama.cpp install lock against a process another thread is starting,
//! which holds a copy of every open file, the lock among them, until it
//! execs. Its own test binary: the copies this test's process keeps open for
//! a moment would otherwise reach every other test's files (a script still
//! open for writing cannot be run on Linux).

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::time::Duration;

use fs2::FileExt;
use runtime::engines::install::remove;
use runtime::engines::llama_cpp::hedos_root;

#[test]
fn a_lock_a_starting_process_holds_for_a_moment_is_taken_once_it_lets_go() {
    let data = std::env::temp_dir().join(format!("hedos-engine-lock-{}", std::process::id()));
    let root = hedos_root(&data);
    std::fs::create_dir_all(&root).unwrap();
    let lock_file = root.join(".lock");
    let held = std::fs::File::create(&lock_file).unwrap();
    held.try_lock_exclusive().unwrap();

    // The process says when it has its copies, then waits to be let go
    // before it execs.
    let (mut forked, forked_end) = std::io::pipe().unwrap();
    let (release_end, mut release) = std::io::pipe().unwrap();
    let (says, waits) = (forked_end.as_raw_fd(), release_end.as_raw_fd());
    // What lets it go exists before it starts and lets it go however this
    // test ends: on the word, or when the word can no longer come.
    let (go, gone) = std::sync::mpsc::channel::<()>();
    let letting_go = std::thread::spawn(move || {
        let _ = gone.recv();
        std::thread::sleep(Duration::from_millis(200));
        let _ = release.write_all(&[0]);
    });
    let mut command = std::process::Command::new("true");
    // SAFETY: between fork and exec the closure only writes to and reads from
    // two pipes the parent keeps open, both async-signal-safe calls.
    unsafe {
        command.pre_exec(move || {
            let mut byte = 0u8;
            libc::write(says, (&raw const byte).cast(), 1);
            libc::read(waits, (&raw mut byte).cast(), 1);
            Ok(())
        });
    }
    // `spawn` returns only once the process has exec'd, so it is started on a
    // thread of its own, as another request would. The parent's copy of the
    // end the process says it on goes with the spawn, so a spawn that fails
    // ends the wait instead of hanging it.
    let starting = std::thread::spawn(move || {
        let spawned = command.spawn();
        drop(forked_end);
        spawned.unwrap().wait().unwrap()
    });
    forked
        .read_exact(&mut [0])
        .expect("the process started and has its copies");
    drop(held);
    let contended = std::fs::File::open(&lock_file)
        .map(|copy| copy.try_lock_exclusive().is_err())
        .unwrap_or(false);
    let _ = go.send(());
    let removed = remove(&data);
    letting_go.join().unwrap();
    starting.join().unwrap();
    drop(release_end);
    let _ = std::fs::remove_dir_all(&data);
    assert!(contended, "the starting process holds the lock");
    assert!(
        removed.is_ok(),
        "taken once the copy closes, not refused as busy: {removed:?}"
    );
}
