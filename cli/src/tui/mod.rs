//! The shelf TUI: a ratatui screen over the same verbs the subcommands expose.
//!
//! `app` holds every piece of state and reduces events to effects without
//! touching the kernel; `tasks` performs the effects that need the kernel, on
//! the runtime; `ui` draws the state; this module owns the terminal and the
//! loop that connects them.

mod app;
mod chat;
mod edit;
mod effect;
mod event;
mod facts;
mod launch;
mod layout;
mod order;
mod pull;
mod state;
mod tasks;
mod text;
mod ui;

use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Interval, MissedTickBehavior};

use self::app::App;
use self::effect::{Effect, HandOff};
use self::event::{Event, Input, Refreshed};
use self::state::UiState;
use self::tasks::{TaskContext, TaskLabel, TaskState};
use crate::commands;
use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::signals;

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
    let context = Arc::new(TaskContext::new(
        Arc::new(session),
        runtime::boot::default_install_service(),
    ));
    let tasks::Snapshot { records, facts } = context.snapshot().await;
    let mut app = App::new(records, facts);
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
    let mut ticks = tokio::time::interval(app::TICK);
    // Ticks missed while something else had the terminal are not owed: a
    // burst of them would age the strip and fire a refresh per 10 s away.
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let outcome = loop {
        match on_terminal(&mut app, &context, &tx, &mut rx, &mut ticks).await {
            Ok(Outcome::HandOff(hand_off)) => {
                let (label, state) = run_hand_off(*hand_off, &context, out).await;
                let sequence = tasks::next_refresh_sequence();
                let tasks::Snapshot { records, facts } = context.snapshot().await;
                app.came_back(
                    Refreshed {
                        sequence,
                        records,
                        facts,
                    },
                    label,
                    state,
                );
                ticks.reset();
            }
            Ok(Outcome::Quit) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    app.remembered().save(&state_dir);
    // A pull or removal is finished rather than cut mid-way; a scan runs to
    // completion inside one poll anyway. Either way, say why the prompt is late.
    if context.busy() || app.busy() {
        out.line("finishing background work…");
    }
    context.settle().await;
    interrupt_guard.abort();
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
) -> Result<Outcome, CliError> {
    let input = Input::spawn(tx.clone());
    // `try_init` installs a panic hook that restores the terminal, but a
    // failure between raw mode and the alternate screen leaves raw mode on.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(terminal_error(error));
        }
    };
    let outcome = drive(&mut terminal, app, context, tx, rx, ticks).await;
    ratatui::restore();
    input.stop();
    outcome
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
    let ran = text::duration(started.elapsed().as_secs() as i64);
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
) -> Result<Outcome, CliError> {
    // The reply streaming into the chat pane; aborting it is how a reply
    // stops, and one is enough since the pane sends one ask at a time.
    let mut ask: Option<JoinHandle<()>> = None;
    loop {
        if app.take_dirty() {
            terminal
                .draw(|frame| ui::draw(frame, app))
                .map_err(terminal_error)?;
        }
        let event = tokio::select! {
            received = rx.recv() => match received {
                Some(event) => event,
                // Unreachable while `tx` lives here; the reducer turns the
                // input thread's own `InputClosed` into a quit.
                None => return Ok(Outcome::Quit),
            },
            _ = ticks.tick() => Event::Tick,
        };
        for effect in app.reduce(event) {
            match effect {
                Effect::Quit => {
                    abort(ask.take());
                    return Ok(Outcome::Quit);
                }
                Effect::HandOff(hand_off) => return Ok(Outcome::HandOff(hand_off)),
                Effect::Spawn(kind) => {
                    let id = tasks::spawn(&kind, context, tx);
                    app.started(id, kind);
                }
                Effect::Refresh => tasks::spawn_refresh(context, tx),
                Effect::Search(query) => tasks::spawn_search(query, context, tx),
                Effect::Plan(provider, reference) => {
                    tasks::spawn_plan(provider, reference, context, tx);
                }
                Effect::Cancel(id) => context.cancel(id),
                Effect::Copy(text) => copy_to_clipboard(&text),
                Effect::Ask {
                    record_id,
                    payload,
                    generation,
                } => {
                    abort(ask.take());
                    ask = Some(tasks::spawn_ask(
                        record_id, payload, generation, context, tx,
                    ));
                }
                Effect::StopAsk => abort(ask.take()),
            }
        }
    }
}

fn abort(handle: Option<JoinHandle<()>>) {
    if let Some(handle) = handle {
        handle.abort();
    }
}

/// Put `text` on the clipboard through OSC 52, which reaches the terminal the
/// user sits at even over ssh or inside tmux (with `set-clipboard on`).
fn copy_to_clipboard(text: &str) {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut stdout = io::stdout();
    let _ = write!(stdout, "\x1b]52;c;{encoded}\x07");
    let _ = stdout.flush();
}

fn terminal_error(error: io::Error) -> CliError {
    CliError::new(format!("terminal error: {error}"))
}
