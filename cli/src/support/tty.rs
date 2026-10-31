//! The terminal's line discipline while a prompt is reading keys.
//!
//! `dialoguer` reads one key at a time, and `console` switches the terminal into
//! raw mode for each of those reads and hands it straight back. Between two keys
//! the line discipline is canonical again, so whatever is left of a paste in the
//! input queue is echoed afresh every time it returns: under tmux a pasted URL
//! is drawn once per character, each copy a character shorter than the one
//! before it. Holding the terminal non-canonical and unechoed for the whole
//! prompt leaves those switches nothing to change, and the paste is drawn once,
//! by the prompt that asked for it.
//!
//! Held modes have to survive the ways a prompt ends. `console` raises SIGINT
//! itself when it reads a Ctrl-C, and nothing would run a guard's `Drop` on the
//! way out, so the signals that would end the process carry a handler that puts
//! the terminal back first. It is installed only where the default action was
//! still in place, so a command that watches a signal itself keeps its own
//! handler and its own shutdown.
//!
//! The signals the process lives through are kept off the prompt's thread while
//! it holds the terminal. `console` waits for a key in `select` or `poll`, which
//! a handled signal interrupts whatever `SA_RESTART` says, and it takes that
//! interruption for a Ctrl-C: a child exiting under tokio's SIGCHLD handler
//! (the daily look for a release is one) would cancel the prompt.

use std::io::IsTerminal;
use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// The signals whose default action ends the process, and which would leave the
/// terminal unechoed if they arrived while a prompt held it.
const FATAL: [libc::c_int; 4] = [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP];

/// The signals a prompt's thread holds back: ones the process lives through,
/// which a handler elsewhere may be waiting for. Another thread takes them, or
/// this one does once the prompt is answered.
const SURVIVED: [libc::c_int; 2] = [libc::SIGCHLD, libc::SIGWINCH];

/// The modes a live prompt replaced, and the terminal to put them back on.
struct Saved {
    fd: RawFd,
    modes: libc::termios,
}

/// The prompt currently holding the terminal, or null when none is. A pointer
/// rather than a lock because the signal handler reads it, and a handler may not
/// wait on anything.
static HELD: AtomicPtr<Saved> = AtomicPtr::new(ptr::null_mut());

/// Hold stdin's terminal non-canonical and unechoed until the returned guard is
/// dropped, so a prompt draws a paste once instead of once per character.
///
/// Inert when stdin is not a terminal, when its modes cannot be read or set, or
/// when another prompt already holds them: in all of those the prompt behaves
/// exactly as it did without a guard.
pub fn hold() -> PromptModes {
    if !std::io::stdin().is_terminal() {
        return PromptModes::inert();
    }
    PromptModes::hold_on(libc::STDIN_FILENO)
}

/// Drop whatever the terminal has sent that nothing has read yet: a reply to
/// a query that came after the asker stopped waiting, or keys pressed while a
/// screen was opening, which would otherwise be read as the first keys.
pub fn discard_input() {
    if !std::io::stdin().is_terminal() {
        return;
    }
    // SAFETY: `tcflush` only reads the descriptor and the queue selector, and
    // stdin stays open for the life of the process.
    unsafe {
        libc::tcflush(libc::STDIN_FILENO, libc::TCIFLUSH);
    }
}

/// A live hold on the terminal's modes, released when it is dropped. It stays
/// on the thread that took it, since that thread's signal mask is what it put
/// back.
pub struct PromptModes {
    held: bool,
    watched: Vec<libc::c_int>,
    mask: Option<libc::sigset_t>,
    on_this_thread: PhantomData<*const ()>,
}

impl PromptModes {
    fn inert() -> Self {
        Self {
            held: false,
            watched: Vec::new(),
            mask: None,
            on_this_thread: PhantomData,
        }
    }

    fn hold_on(fd: RawFd) -> Self {
        let mut modes = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `tcgetattr` writes a `termios` into the buffer it is given and
        // reports failure through its return value, in which case the buffer is
        // left untouched and never read.
        if unsafe { libc::tcgetattr(fd, modes.as_mut_ptr()) } != 0 {
            return Self::inert();
        }
        // SAFETY: only read once `tcgetattr` reported that it filled the buffer.
        let original = unsafe { modes.assume_init() };

        let saved = Box::into_raw(Box::new(Saved {
            fd,
            modes: original,
        }));
        if HELD
            .compare_exchange(ptr::null_mut(), saved, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // SAFETY: the exchange failed, so nothing else ever saw this box and
            // this is its only owner.
            drop(unsafe { Box::from_raw(saved) });
            return Self::inert();
        }
        let watched = FATAL.into_iter().filter(|signal| watch(*signal)).collect();

        let mut quiet = original;
        // ISIG goes with the rest: `console` clears it for the length of every
        // read anyway, so leaving it on would only mean a Ctrl-C landing as a
        // signal or as a keypress depending on which microsecond it arrived in.
        quiet.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        quiet.c_cc[libc::VMIN] = 1;
        quiet.c_cc[libc::VTIME] = 0;
        // SAFETY: `quiet` is this terminal's own modes with three flags cleared.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
            let mut failed = Self {
                held: true,
                watched,
                mask: None,
                on_this_thread: PhantomData,
            };
            failed.release();
            return Self::inert();
        }
        Self {
            held: true,
            watched,
            mask: hold_back_survived(),
            on_this_thread: PhantomData,
        }
    }

    /// Put the terminal back the way the prompt found it. Pending input goes
    /// with it: what a prompt did not read was typed at that prompt, and letting
    /// it through would echo it at the next one, or drive it.
    fn release(&mut self) {
        if !self.held {
            return;
        }
        self.held = false;
        let held = HELD.load(Ordering::SeqCst);
        if !held.is_null() {
            // SAFETY: `HELD` holds the box this guard published, and it is
            // cleared below before the box is freed.
            unsafe {
                libc::tcsetattr((*held).fd, libc::TCSAFLUSH, &(*held).modes);
            }
        }
        for signal in self.watched.drain(..) {
            // SAFETY: `watch` only reports a signal it installed the handler for,
            // and it only installs one where the default action was in place.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
            }
        }
        let held = HELD.swap(ptr::null_mut(), Ordering::SeqCst);
        if !held.is_null() {
            // SAFETY: the swap took the pointer out of reach of the handler, and
            // this guard is the only owner of what it published.
            drop(unsafe { Box::from_raw(held) });
        }
        if let Some(mask) = self.mask.take() {
            // SAFETY: `mask` is the set `pthread_sigmask` reported for this
            // thread, which the guard never left; a signal held back meanwhile
            // is delivered now.
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &mask, ptr::null_mut());
            }
        }
    }

    #[cfg(test)]
    fn is_held(&self) -> bool {
        self.held
    }
}

impl Drop for PromptModes {
    fn drop(&mut self) {
        self.release();
    }
}

/// Block [`SURVIVED`] on this thread, and return the mask it had, or `None` when
/// it could not be changed and the prompt reads as it did before.
fn hold_back_survived() -> Option<libc::sigset_t> {
    let mut survived = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    let mut previous = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` fills the set before `sigaddset` adds to it, and
    // `pthread_sigmask` writes the thread's mask into `previous`, which is only
    // read once it reported success.
    unsafe {
        libc::sigemptyset(survived.as_mut_ptr());
        for signal in SURVIVED {
            libc::sigaddset(survived.as_mut_ptr(), signal);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, survived.as_ptr(), previous.as_mut_ptr()) != 0 {
            return None;
        }
        Some(previous.assume_init())
    }
}

/// Install [`put_back`] for `signal`, and report whether it stayed. A handler
/// already in place means something else is watching and the process is not
/// going to die of it, so that handler is left alone and the guard's `Drop` does
/// the restoring.
fn watch(signal: libc::c_int) -> bool {
    // SAFETY: `put_back` has the C handler signature, and any disposition other
    // than the default one is put straight back.
    unsafe {
        let previous = libc::signal(signal, put_back as *const () as libc::sighandler_t);
        if previous == libc::SIG_DFL {
            return true;
        }
        if previous != libc::SIG_ERR {
            libc::signal(signal, previous);
        }
        false
    }
}

/// Restore the held terminal and die of `signal` the way the process would have
/// without a prompt in the way.
extern "C" fn put_back(signal: libc::c_int) {
    let held = HELD.load(Ordering::SeqCst);
    if !held.is_null() {
        // SAFETY: `HELD` is either null or a live `Saved`, cleared before the box
        // is freed, and `tcsetattr` is safe to call from a handler.
        unsafe {
            libc::tcsetattr((*held).fd, libc::TCSANOW, &(*held).modes);
        }
    }
    // SAFETY: the handler is only installed where the default action was still
    // in place, so the process was going to end here anyway; it ends here now,
    // with the terminal as the user left it.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Mutex, MutexGuard};

    /// One terminal, one hold: the tests take turns rather than racing each
    /// other for [`HELD`].
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn in_turn() -> MutexGuard<'static, ()> {
        ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct Pty {
        controller: RawFd,
        terminal: RawFd,
    }

    impl Pty {
        fn open() -> Self {
            let mut controller = 0;
            let mut terminal = 0;
            // SAFETY: `openpty` fills the two descriptors it is given and takes
            // the terminal's name and modes as optional, which null declines.
            let opened = unsafe {
                libc::openpty(
                    &mut controller,
                    &mut terminal,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            assert_eq!(opened, 0, "a test needs a terminal to hold");
            Self {
                controller,
                terminal,
            }
        }

        fn modes(&self) -> libc::termios {
            let mut modes = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: as in `hold_on`, the buffer is only read once it is filled.
            assert_eq!(
                unsafe { libc::tcgetattr(self.terminal, modes.as_mut_ptr()) },
                0
            );
            unsafe { modes.assume_init() }
        }

        fn echoes(&self) -> bool {
            self.modes().c_lflag & (libc::ICANON | libc::ECHO) != 0
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            // SAFETY: both descriptors came from `openpty` and are closed once.
            unsafe {
                libc::close(self.controller);
                libc::close(self.terminal);
            }
        }
    }

    #[test]
    fn a_held_terminal_stops_echoing_and_gets_its_modes_back() {
        let _turn = in_turn();
        let pty = Pty::open();
        let before = pty.modes();
        assert!(pty.echoes(), "a fresh terminal echoes what is typed at it");

        let modes = PromptModes::hold_on(pty.terminal);
        assert!(modes.is_held());
        assert!(
            !pty.echoes(),
            "the prompt draws the paste, the terminal does not"
        );

        drop(modes);
        assert!(pty.echoes());
        assert_eq!(pty.modes().c_lflag, before.c_lflag);
    }

    #[test]
    fn a_second_hold_leaves_the_first_one_alone() {
        let _turn = in_turn();
        let pty = Pty::open();
        let outer = PromptModes::hold_on(pty.terminal);
        assert!(outer.is_held());

        let inner = PromptModes::hold_on(pty.terminal);
        assert!(!inner.is_held(), "one prompt holds the terminal at a time");
        drop(inner);
        assert!(!pty.echoes(), "and dropping the second did not release it");

        drop(outer);
        assert!(pty.echoes());
    }

    /// SIGCHLDs handled on [`PROMPTING`], the thread holding the terminal:
    /// other tests' children end on other threads meanwhile.
    static CHILD_SIGNALS: AtomicUsize = AtomicUsize::new(0);
    static PROMPTING: AtomicUsize = AtomicUsize::new(0);
    static PREVIOUS_CHILD_ACTION: AtomicPtr<libc::sigaction> = AtomicPtr::new(ptr::null_mut());

    /// Count a SIGCHLD handled on the prompting thread, and pass every one on
    /// to whatever handled it before, so a test elsewhere waiting on a child
    /// still hears of it.
    extern "C" fn count_child(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        context: *mut libc::c_void,
    ) {
        // SAFETY: `pthread_self` names the calling thread and may be called
        // from a handler.
        if unsafe { libc::pthread_self() } as usize == PROMPTING.load(Ordering::SeqCst) {
            CHILD_SIGNALS.fetch_add(1, Ordering::SeqCst);
        }
        let previous = PREVIOUS_CHILD_ACTION.load(Ordering::SeqCst);
        if previous.is_null() {
            return;
        }
        // SAFETY: `previous` is the action `sigaction` reported, leaked so it
        // is never freed, and called the way it was registered to be.
        unsafe {
            let action = (*previous).sa_sigaction;
            if action == libc::SIG_DFL || action == libc::SIG_IGN {
                return;
            }
            if (*previous).sa_flags & libc::SA_SIGINFO != 0 {
                let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
                    std::mem::transmute(action);
                handler(signal, info, context);
            } else {
                let handler: extern "C" fn(libc::c_int) = std::mem::transmute(action);
                handler(signal);
            }
        }
    }

    #[test]
    fn a_child_ending_while_a_prompt_waits_neither_wakes_nor_is_lost() {
        let _turn = in_turn();
        let pty = Pty::open();

        // SAFETY: `sigaction` is plain data, for which all zeroes is a valid
        // value. `previous` is leaked so a handler still running on another
        // thread never reads it freed.
        let previous: &'static mut libc::sigaction =
            Box::leak(Box::new(unsafe { std::mem::zeroed::<libc::sigaction>() }));
        // SAFETY: with no new action, `sigaction` only writes the current one
        // into `previous`.
        unsafe {
            assert_eq!(
                libc::sigaction(libc::SIGCHLD, ptr::null(), &mut *previous),
                0
            );
        }
        // Published before the counting handler goes in, so a child ending in
        // between is still passed on.
        PREVIOUS_CHILD_ACTION.store(&mut *previous, Ordering::SeqCst);
        // SAFETY: `pthread_self` names the calling thread, alive throughout.
        let prompting = unsafe { libc::pthread_self() } as usize;
        PROMPTING.store(prompting, Ordering::SeqCst);
        // SAFETY: as above, all zeroes is a valid `sigaction`.
        let mut counting = unsafe { std::mem::zeroed::<libc::sigaction>() };
        counting.sa_sigaction = count_child as *const () as libc::sighandler_t;
        counting.sa_flags = libc::SA_SIGINFO;
        // SAFETY: `counting` is initialized, its mask emptied first.
        unsafe {
            libc::sigemptyset(&mut counting.sa_mask);
            assert_eq!(
                libc::sigaction(libc::SIGCHLD, &counting, ptr::null_mut()),
                0
            );
        }
        let _restore = RestoreChildAction(previous);

        let modes = PromptModes::hold_on(pty.terminal);
        assert!(modes.is_held());
        // Read once SIGCHLD is held back here, so a child of another test
        // cannot land on this thread after it.
        let before = CHILD_SIGNALS.load(Ordering::SeqCst);
        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            // SAFETY: the prompting thread is still waiting in `select` below.
            unsafe { libc::pthread_kill(prompting as libc::pthread_t, libc::SIGCHLD) }
        });

        // The wait `console` makes for a key: nothing is typed, so only a
        // signal could end it before the timeout.
        let mut timeout = libc::timeval {
            tv_sec: 0,
            tv_usec: 500_000,
        };
        // SAFETY: `fds` is zeroed and then filled by `FD_SET` with one open
        // descriptor below `FD_SETSIZE`, and `timeout` lives for the call.
        let woke = unsafe {
            let mut fds = std::mem::zeroed::<libc::fd_set>();
            libc::FD_ZERO(&mut fds);
            libc::FD_SET(pty.terminal, &mut fds);
            libc::select(
                pty.terminal + 1,
                &mut fds,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut timeout,
            )
        };
        let error = std::io::Error::last_os_error();
        assert_eq!(sender.join().ok(), Some(0));
        assert_eq!(
            woke, 0,
            "the wait ran to its timeout instead of ending early: {error}"
        );
        assert_eq!(
            CHILD_SIGNALS.load(Ordering::SeqCst),
            before,
            "the signal waited"
        );

        drop(modes);
        assert!(
            CHILD_SIGNALS.load(Ordering::SeqCst) > before,
            "and arrived once the prompt let go"
        );
    }

    /// Put SIGCHLD's action back however the test ends.
    struct RestoreChildAction(&'static libc::sigaction);

    impl Drop for RestoreChildAction {
        fn drop(&mut self) {
            // SAFETY: the action is what `sigaction` reported for SIGCHLD.
            unsafe {
                libc::sigaction(libc::SIGCHLD, self.0, ptr::null_mut());
            }
        }
    }

    #[test]
    fn nothing_that_is_not_a_terminal_is_held() {
        let _turn = in_turn();
        let mut ends = [0; 2];
        // SAFETY: `pipe` fills the pair of descriptors it is given.
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        let modes = PromptModes::hold_on(ends[0]);
        assert!(!modes.is_held(), "a pipe has no line discipline to hold");
        // SAFETY: both descriptors came from `pipe` and are closed once.
        unsafe {
            libc::close(ends[0]);
            libc::close(ends[1]);
        }
    }
}
