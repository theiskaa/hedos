//! The shelf TUI: a ratatui screen over the same verbs the subcommands expose.
//!
//! `app` holds every piece of state and reduces events to effects without
//! touching the kernel; `tasks` performs the effects that need the kernel, on
//! the runtime; `ui` draws the state; this module owns the terminal and the
//! loop that connects them.

mod app;
mod effect;
mod event;
pub(crate) mod facts;
mod layout;
mod pull;
mod tasks;
mod text;
mod ui;

use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::time::Interval;

pub use self::app::App;
use self::effect::Effect;
use self::event::Event;
use self::tasks::TaskContext;
use crate::error::CliError;
use crate::support::output::Out;
use crate::support::session::Session;

/// Run `app` over `session` on the terminal until it asks to quit.
pub async fn run(mut app: App, session: Arc<Session>, out: &Out) -> Result<(), CliError> {
    let context = Arc::new(TaskContext::new(
        session,
        runtime::boot::default_install_service(),
    ));
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
    if app.busy() {
        // A scan runs to completion inside one poll, so the runtime waits for
        // it before the process can exit; say why the prompt is late.
        out.line("finishing background work…");
    }
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
            }
        }
    }
}

fn terminal_error(error: io::Error) -> CliError {
    CliError::new(format!("terminal error: {error}"))
}
