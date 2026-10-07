//! Signal handling: await Ctrl-C so long-running commands (`serve`, `pull`)
//! can stop cleanly, and turn a termination (SIGTERM, or SIGHUP from a closed
//! terminal) into the same orderly exit. The model servers a command starts
//! live in their own process groups and are stopped when the kernel holding
//! them is dropped, so a process the default disposition kills outright
//! leaves every one of them running. A termination the process was started
//! ignoring, as `nohup` starts it ignoring SIGHUP, stays ignored.

use std::sync::OnceLock;

use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::watch;

/// The termination signal delivered, once one is.
static TERMINATION: OnceLock<watch::Receiver<Option<i32>>> = OnceLock::new();

/// Resolve when the user presses Ctrl-C (SIGINT). If the handler can't be
/// installed, never resolve — a server must not treat a failed install as an
/// immediate shutdown, and a cancellable command should keep running.
pub async fn wait_for_ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Ctrl-C presses, each one observed. One listener is held across presses, so
/// a press that lands while an earlier one is being acted on is kept for the
/// next wait rather than lost between two listeners.
pub struct Interrupts(Option<Signal>);

impl Interrupts {
    /// Listen for Ctrl-C from now on. Must be called from within the Tokio
    /// runtime.
    pub fn new() -> Self {
        Self(signal(SignalKind::interrupt()).ok())
    }

    /// Resolve on the next press; never, when the handler could not be
    /// installed.
    pub async fn next(&mut self) {
        if let Some(signal) = &mut self.0
            && signal.recv().await.is_some()
        {
            return;
        }
        std::future::pending::<()>().await;
    }
}

/// Take over SIGTERM and SIGHUP for the rest of the process, so either one is
/// observed through [`terminated`] instead of killing the process outright.
/// One the process was started ignoring is left ignored. Must be called from
/// within the Tokio runtime; a second call does nothing.
pub fn watch_termination() {
    TERMINATION.get_or_init(|| {
        let (sender, receiver) = watch::channel(None);
        // Installed here rather than in the task, so a signal that arrives
        // before the task first runs is already caught.
        let watched = |number: libc::c_int, kind: SignalKind| {
            if ignored(number) {
                None
            } else {
                signal(kind).ok()
            }
        };
        let terminate = watched(libc::SIGTERM, SignalKind::terminate());
        let hangup = watched(libc::SIGHUP, SignalKind::hangup());
        tokio::spawn(async move {
            let number = tokio::select! {
                () = delivered(terminate) => libc::SIGTERM,
                () = delivered(hangup) => libc::SIGHUP,
            };
            let _ = sender.send(Some(number));
        });
        receiver
    });
}

/// Resolve with the signal's number once a termination is delivered. Never
/// resolves when [`watch_termination`] was not called.
pub async fn terminated() -> i32 {
    if let Some(receiver) = TERMINATION.get() {
        let mut receiver = receiver.clone();
        if let Ok(number) = receiver.wait_for(Option::is_some).await
            && let Some(number) = *number
        {
            return number;
        }
    }
    std::future::pending().await
}

/// The termination delivered so far, if any.
pub fn termination() -> Option<i32> {
    TERMINATION.get().and_then(|receiver| *receiver.borrow())
}

/// Resolve when `signal` is delivered; never, when its handler could not be
/// installed.
async fn delivered(signal: Option<Signal>) {
    if let Some(mut signal) = signal
        && signal.recv().await.is_some()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// Whether the signal `number` is ignored. Read without changing it, before
/// any handler of this process replaces it.
fn ignored(number: libc::c_int) -> bool {
    let mut current = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: with a null new action, `sigaction` only writes the current one
    // into the buffer it is given and reports failure through its return
    // value, in which case the buffer is never read.
    let read = unsafe { libc::sigaction(number, std::ptr::null(), current.as_mut_ptr()) } == 0;
    // SAFETY: only read once `sigaction` reported that it filled the buffer.
    read && unsafe { current.assume_init() }.sa_sigaction == libc::SIG_IGN
}
