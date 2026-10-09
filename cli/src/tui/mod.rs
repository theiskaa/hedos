//! The shelf TUI: a ratatui screen over the same verbs the subcommands expose.
//!
//! `app` holds every piece of state and reduces events to effects without
//! touching the kernel; `tasks` performs the effects that need the kernel, on
//! the runtime; `ui` draws the state; this module owns the terminal and the
//! loop that connects them.

mod app;
mod bench;
pub(crate) mod bench_view;
mod chat;
mod edit;
mod effect;
mod event;
pub(crate) mod facts;
mod jobs;
mod keymap;
mod koala;
mod launch;
mod layout;
mod markup;
mod motion;
mod order;
mod palette;
mod pixel;
mod pull;
mod pulls;
mod state;
mod stop;
mod strip;
mod tasks;
#[cfg(test)]
mod testing;
mod text;
mod ui;
mod wrap;

use std::io::{self, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

use tokio::sync::mpsc;
use tokio::time::{Interval, MissedTickBehavior};

use self::app::App;
use self::effect::{Effect, HandOff};
use self::event::{Event, Input};
use self::state::UiState;
use self::tasks::{TaskContext, TaskLabel, TaskState};
use crate::commands;
use crate::error::CliError;
use crate::support::clock;
use crate::support::output::{self, Out};
use crate::support::session::Session;
use crate::support::signals;

/// The pulls screen's rows, for a test that holds every surface's reading of
/// one job side by side.
#[cfg(test)]
pub(crate) use self::jobs::rows as pull_rows;

/// Why `drive` returned.
enum Outcome {
    Quit,
    HandOff(Box<HandOff>),
}

/// Run the UI over `session` on the terminal until it asks to quit. When it
/// hands the terminal to something else, that runs here in between, and the
/// UI comes back with a fresh snapshot once it is over.
pub async fn run(session: Session, out: &Out) -> Result<(), CliError> {
    session.shelf_or_discover().await?;
    let state_dir = session.dirs.sub("ui");
    let context = Arc::new(TaskContext::new(Arc::new(session)));
    let pull_settings = &context.session().settings.pull;
    runtime::install::collect_ended(&context.pull_store(), pull_settings);
    // A pull whose worker died while the machine slept is the common way one
    // stops, and the screen is where the user finds out. With auto-resume on,
    // they find it going again rather than waiting to be told to carry on.
    if pull_settings.auto_resume {
        runtime::install::resume_all(&context.pull_store());
    }
    let tasks::Snapshot { records, facts } = context.snapshot().await;
    let mut app = App::new(records, facts);
    app.depth = palette::Depth::detect();
    app.ground = palette::Ground::detect();
    app.motion = motion::Motion::from_env();
    // One clock for the whole run: a hand-off steps out of the loop and back
    // in, and the intro must not play again when it does.
    let started = Instant::now();
    app.restore(&UiState::load(&state_dir));
    // `shelf_or_discover` already scanned an empty shelf; with still nothing
    // to show, the useful first screen is what could be pulled.
    app.offer_pull_when_empty();
    // Ctrl-C reaches the UI as a key in raw mode, and the hand-offs in cooked
    // mode either watch for it themselves or leave it to their child; either
    // way it must never kill this process with unsaved state and pulls in
    // flight. Holding the handler for the whole run makes that true from the
    // first frame, not only after the first serve installed one.
    let interrupt_guard = tokio::spawn(async {
        loop {
            signals::wait_for_ctrl_c().await;
        }
    });
    let (tx, mut rx) = mpsc::unbounded_channel();
    // A termination quits the way a closed input does, so the screen's state
    // is saved, the terminal restored, and the servers stopped on the way out.
    let termination_guard = tokio::spawn({
        let tx = tx.clone();
        async move {
            signals::terminated().await;
            let _ = tx.send(Event::InputClosed);
        }
    });
    let mut ticks = tokio::time::interval(app::TICK);
    // Ticks missed while something else had the terminal are not owed: a
    // burst of them would age the strip and fire a refresh per 10 s away.
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // The job directory is read before the first frame rather than on the
    // first cadence, so a download under way is on the strip when the screen
    // appears, which is the point of it surviving the terminal.
    tasks::spawn_pulls(&context, &tx);
    let terminal_modes = TerminalModes::capture();
    // A launch hand-off cut by a termination leaves its harness running on
    // the terminal, whose modes are then its own to put back.
    let mut harness_kept_terminal = false;
    let outcome = loop {
        match on_terminal(
            &mut app,
            &context,
            &tx,
            &mut rx,
            &mut ticks,
            &terminal_modes,
            started,
        )
        .await
        {
            Ok(Outcome::HandOff(hand_off)) => {
                // A served gateway stops itself on a termination, draining
                // what is in flight. Any other hand-off may be waiting on the
                // user's next line, which a termination must not wait for.
                let launches = matches!(*hand_off, HandOff::Launch { .. });
                let ran = if matches!(*hand_off, HandOff::Serve) {
                    run_hand_off(*hand_off, &context, out).await
                } else {
                    tokio::select! {
                        ran = run_hand_off(*hand_off, &context, out) => ran,
                        _ = signals::terminated() => {
                            harness_kept_terminal = launches;
                            break Ok(());
                        }
                    }
                };
                if signals::termination().is_some() {
                    break Ok(());
                }
                let (label, state) = ran;
                let sequence = tasks::next_refresh_sequence();
                let snapshot = context.snapshot().await.stamped(sequence);
                app.came_back(snapshot, label, state);
                ticks.reset();
            }
            Ok(Outcome::Quit) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    app.remembered().save(&state_dir);
    // A removal is finished rather than cut between deleting and forgetting; a
    // scan runs to completion inside one poll anyway. A download is not waited
    // for at all: it belongs to a worker that outlives this process.
    if context.busy() || app.busy() {
        out.line("finishing background work…");
    }
    context.settle().await;
    interrupt_guard.abort();
    termination_guard.abort();
    if !harness_kept_terminal {
        terminal_modes.restore();
    }
    outcome
}

/// Own the terminal and the input thread for one stretch of the UI: set
/// both up, drive until something ends the stretch, and give both back.
async fn on_terminal(
    app: &mut App,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
    rx: &mut mpsc::UnboundedReceiver<Event>,
    ticks: &mut Interval,
    terminal_modes: &TerminalModes,
    started: Instant,
) -> Result<Outcome, CliError> {
    // Whatever the last hand-off left the terminal in, the UI starts from
    // the modes the user's shell had, and those are what `restore` returns.
    terminal_modes.restore();
    let input = Input::spawn(tx.clone());
    // `try_init` installs a panic hook that restores the terminal, but a
    // failure between raw mode and the alternate screen leaves raw mode on.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            restore_screen();
            return Err(terminal_error(error));
        }
    };
    let reporting = Reporting::enable();
    let outcome = drive(&mut terminal, app, context, tx, rx, ticks, started).await;
    // Dropped, the terminal shows the cursor again and reports a failure to
    // stderr, which on a closed terminal fails in turn and aborts. One that
    // cannot be shown the cursor is gone, so it is let go without that.
    if terminal.show_cursor().is_err() {
        std::mem::forget(terminal);
    } else {
        drop(terminal);
    }
    drop(reporting);
    restore_screen();
    // Only a hand-off needs the reader gone: on the way out it is left to
    // end, so one stuck on a terminal that hung up never holds the exit.
    if matches!(outcome, Ok(Outcome::HandOff(_))) {
        input.hand_over();
    } else {
        drop(input);
    }
    // Keys read in the moment before the reader stopped would otherwise act
    // on the UI when it comes back, in a screen they were not typed at.
    let mut kept = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if !matches!(event, Event::Key(_)) {
            kept.push(event);
        }
    }
    for event in kept {
        let _ = tx.send(event);
    }
    outcome
}

/// Leave raw mode and the alternate screen and show the cursor, as
/// `ratatui::restore` and the terminal's own drop do, but without reporting a
/// failure: the terminal may have been closed, and the report would go to it.
fn restore_screen() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

/// The terminal's line discipline as the UI found it. Raw mode is switched
/// on and off around every stretch of the UI, and crossterm's idea of the
/// "original" modes is whatever it sees when switching on; a hand-off that
/// died with raw mode still set would otherwise become the baseline that
/// quitting restores.
struct TerminalModes(Option<libc::termios>);

impl TerminalModes {
    fn capture() -> Self {
        let mut modes = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: `tcgetattr` writes a `termios` into the buffer it is given
        // and reports failure through its return value, in which case the
        // buffer is left untouched and never read.
        let captured = unsafe { libc::tcgetattr(libc::STDIN_FILENO, modes.as_mut_ptr()) } == 0;
        // SAFETY: only read once `tcgetattr` reported that it filled the buffer.
        Self(captured.then(|| unsafe { modes.assume_init() }))
    }

    fn restore(&self) {
        if let Some(modes) = &self.0 {
            // SAFETY: `modes` is a `termios` that `tcgetattr` produced for
            // this same descriptor.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, modes);
            }
        }
    }
}

/// Mouse reporting for the wheel, which scrolls the transcript and the
/// shelf; without capture the terminal would scroll its own (empty) history.
/// And bracketed paste, so a paste arrives whole: a text with line breaks
/// keeps them where a field takes them, and a break never reads as enter.
/// Held as a guard because ratatui's panic hook restores the screen but
/// knows nothing about either, and a shell left reporting the mouse prints a
/// code on every move.
struct Reporting;

impl Reporting {
    fn enable() -> Self {
        let _ = execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);
        Self
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    }
}

/// Run `hand_off` in the foreground and describe how it went as a task row.
async fn run_hand_off(
    hand_off: HandOff,
    context: &Arc<TaskContext>,
    out: &Out,
) -> (TaskLabel, TaskState) {
    let session = context.session();
    let label = hand_off.label(session.settings.gateway.port);
    let started = Instant::now();
    let result: Result<Option<ExitStatus>, CliError> = match &hand_off {
        HandOff::Launch {
            harness,
            program,
            record,
        } => commands::launch::launch(session, harness, program, record, &[], out)
            .await
            .map(Some),
        HandOff::Chat { record } => commands::chat::chat(session, record, None, None, out)
            .await
            .map(|()| None),
        HandOff::Serve => commands::serve::serve(session, None, out)
            .await
            .map(|()| None),
    };
    let ran = clock::duration(started.elapsed().as_secs() as i64);
    let state = match result {
        Ok(None) => TaskState::Done(format!("ran {ran}")),
        Ok(Some(status)) => match status.code() {
            Some(0) => TaskState::Done(format!("ran {ran}")),
            Some(code) => TaskState::Done(format!("ran {ran} · exit {code}")),
            None => TaskState::Failed(format!("ran {ran} · {status}")),
        },
        Err(error) => {
            // The answer's screen is about to be held; the reason belongs on it.
            out.err(&error.message);
            TaskState::Failed(error.message)
        }
    };
    (label, state)
}

async fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    context: &Arc<TaskContext>,
    tx: &mpsc::UnboundedSender<Event>,
    rx: &mut mpsc::UnboundedReceiver<Event>,
    ticks: &mut Interval,
    started: Instant,
) -> Result<Outcome, CliError> {
    let clock = |app: &mut App| {
        app.set_clock(started.elapsed().as_millis() as u64);
        app.motion.set_wall(kernel::time::now_millis());
    };
    let mut last_frame = tokio::time::Instant::now();
    loop {
        clock(app);
        if app.take_dirty()
            && let Err(error) = terminal.draw(|frame| ui::draw(frame, app))
        {
            // A terminal that hung up takes no more frames. Its hang-up
            // quits the screen as quietly as a closed input does, whichever
            // of the two is noticed first.
            if output::terminal_gone(&error) {
                return Ok(Outcome::Quit);
            }
            return Err(terminal_error(error));
        }
        let event = tokio::select! {
            received = rx.recv() => match received {
                Some(event) => event,
                // Unreachable while `tx` lives here; the reducer turns the
                // input thread's own `InputClosed` into a quit.
                None => return Ok(Outcome::Quit),
            },
            _ = ticks.tick() => Event::Tick,
            () = next_frame(app.frame_due(), last_frame) => {
                last_frame = tokio::time::Instant::now();
                Event::Frame
            }
        };
        clock(app);
        for effect in app.reduce(event) {
            match effect {
                Effect::Quit => return Ok(Outcome::Quit),
                Effect::HandOff(hand_off) => return Ok(Outcome::HandOff(hand_off)),
                Effect::Spawn(kind) => {
                    let id = tasks::spawn(&kind, context, tx);
                    app.started(id, kind);
                }
                Effect::Refresh => tasks::spawn_refresh(context, tx),
                Effect::PollPulls => tasks::spawn_pulls(context, tx),
                Effect::PollHistory(job) => tasks::spawn_history(job, context, tx),
                Effect::StartPull(plan) => tasks::spawn_start_pull(*plan, context, tx),
                Effect::ControlPull(action, job) => {
                    tasks::spawn_pull_control(action, job, context, tx);
                }
                Effect::Search(query) => tasks::spawn_search(query, context, tx),
                Effect::Plan(provider, reference, ask) => {
                    tasks::spawn_plan(provider, reference, ask, context, tx);
                }
                Effect::Copy(text) => copy_to_clipboard(&text),
                Effect::Ask {
                    record_id,
                    capability,
                    payload,
                    generation,
                } => tasks::spawn_ask(record_id, capability, payload, generation, context, tx),
                Effect::StopAsk => context.stop_ask(),
                Effect::StartBench(plan, generation) => {
                    tasks::spawn_bench(*plan, generation, context, tx);
                }
                Effect::StopBench => context.stop_bench(),
            }
        }
    }
}

/// Wait until `due` after the `last` frame, or forever when nothing on
/// screen moves on its own.
async fn next_frame(due: Option<Duration>, last: tokio::time::Instant) {
    match due {
        Some(due) => tokio::time::sleep_until(last + due).await,
        None => std::future::pending().await,
    }
}

/// Put `text` on the pasteboard both ways, since each reaches one the other
/// cannot: `pbcopy` this machine's, whatever terminal or multiplexer sits in
/// between; OSC 52 the terminal's, which over ssh is the one the user sits
/// at (tmux relays it only with `set-clipboard on`).
fn copy_to_clipboard(text: &str) {
    pbcopy(text);
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut stdout = io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

/// `text` through `pbcopy`; whether it took it.
fn pbcopy(text: &str) -> bool {
    let Ok(mut child) = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    written && child.wait().is_ok_and(|status| status.success())
}

fn terminal_error(error: io::Error) -> CliError {
    CliError::new(format!("terminal error: {error}"))
}
