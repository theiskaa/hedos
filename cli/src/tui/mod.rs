//! The shelf TUI: a ratatui screen over the same verbs the subcommands expose.
//!
//! `app` holds every piece of state and reduces events to effects without
//! touching the kernel; `tasks` performs the effects that need the kernel, on
//! the runtime; `ui` draws the state; this module owns the terminal and the
//! loop that connects them.

mod app;
mod effect;
mod event;
mod facts;
mod layout;
mod order;
mod pull;
mod state;
mod tasks;
mod text;
mod ui;

use std::io::{self, Write};
use std::sync::Arc;

use base64::Engine;

use tokio::sync::mpsc;
use tokio::time::Interval;

use self::app::App;
use self::effect::Effect;
use self::event::Event;
use self::state::UiState;
use self::tasks::TaskContext;
use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;

/// Run the UI over `session` on the terminal until it asks to quit.
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
    let (tx, mut rx) = mpsc::unbounded_channel();
    // The input thread owns the only strong sender: when it dies the channel
    // closes and the loop ends, instead of ticking on with no way to quit.
    let weak = tx.downgrade();
    event::spawn_input(tx);
    let mut ticks = tokio::time::interval(app::TICK);

    // `try_init` installs a panic hook that restores the terminal, but a
    // failure between raw mode and the alternate screen leaves raw mode on.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(terminal_error(error));
        }
    };
    let outcome = drive(
        &mut terminal,
        &mut app,
        &context,
        &weak,
        &mut rx,
        &mut ticks,
    )
    .await;
    ratatui::restore();
    app.remembered().save(&state_dir);
    // A pull or removal is finished rather than cut mid-way; a scan runs to
    // completion inside one poll anyway. Either way, say why the prompt is late.
    if context.busy() || app.busy() {
        out.line("finishing background work…");
    }
    context.settle().await;
    outcome
}

async fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    context: &Arc<TaskContext>,
    tx: &mpsc::WeakUnboundedSender<Event>,
    rx: &mut mpsc::UnboundedReceiver<Event>,
    ticks: &mut Interval,
) -> Result<(), CliError> {
    loop {
        if app.take_dirty() {
            terminal
                .draw(|frame| ui::draw(frame, app))
                .map_err(terminal_error)?;
        }
        let event = tokio::select! {
            received = rx.recv() => match received {
                Some(event) => event,
                None => return Ok(()),
            },
            _ = ticks.tick() => Event::Tick,
        };
        for effect in app.reduce(event) {
            let Some(tx) = tx.upgrade() else {
                return Ok(());
            };
            match effect {
                Effect::Quit => return Ok(()),
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
            }
        }
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
