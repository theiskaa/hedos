//! The shelf TUI: a ratatui screen over the same verbs the subcommands expose.
//!
//! `app` holds every piece of state and reduces events to effects without
//! touching the kernel; `ui` draws that state; this module owns the terminal
//! and the loop that connects them.

mod app;
mod effect;
mod event;
pub(crate) mod facts;
mod layout;
mod text;
mod ui;

use std::io;
use std::time::Duration;

use tokio::sync::mpsc;

pub use self::app::App;
use self::effect::Effect;
use self::event::Event;
use crate::error::CliError;

/// How often the loop wakes without input, for animations and expiry.
const TICK: Duration = Duration::from_millis(250);

/// Run `app` on the terminal until it asks to quit.
pub async fn run(mut app: App) -> Result<(), CliError> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    event::spawn_input(tx);
    let mut ticks = tokio::time::interval(TICK);

    // `try_init` installs a panic hook that restores the terminal, but a
    // failure between raw mode and the alternate screen leaves raw mode on.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(terminal_error(error));
        }
    };
    let outcome = drive(&mut terminal, &mut app, &mut rx, &mut ticks).await;
    ratatui::restore();
    outcome
}

async fn drive(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Event>,
    ticks: &mut tokio::time::Interval,
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
                // The input thread is gone; nothing can drive the UI anymore.
                None => return Ok(()),
            },
            _ = ticks.tick() => Event::Tick,
        };
        if app.reduce(event).contains(&Effect::Quit) {
            return Ok(());
        }
    }
}

fn terminal_error(error: io::Error) -> CliError {
    CliError::new(format!("terminal error: {error}"))
}
