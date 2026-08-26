//! Everything that wakes the loop: keys from the terminal, the tick, progress
//! from background tasks, and a refreshed shelf. Keys are translated into the
//! app's own [`Key`] here so the reducer never sees terminal types.

use std::thread;
use std::time::Duration;

use kernel::install::plan::{InstallPlan, InstallSearchHit};
use kernel::records::ModelRecord;
use ratatui::crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use super::facts::Facts;
use super::tasks::TaskEvent;

/// One wake-up of the loop.
#[derive(Debug, Clone)]
pub enum Event {
    /// A key the user pressed.
    Key(Key),
    /// The terminal changed size.
    Resize,
    /// The periodic tick.
    Tick,
    /// A background task moved.
    Task(TaskEvent),
    /// The shelf and facts were re-read.
    Refreshed(Refreshed),
    /// A provider search came back.
    Searched(Searched),
    /// An install plan came back.
    Planned(Planned),
}

/// The hits for a query, or why there are none.
#[derive(Debug, Clone)]
pub struct Searched {
    pub query: String,
    pub hits: Vec<InstallSearchHit>,
    pub note: Option<String>,
}

/// The plan for a reference, or why it could not be made.
#[derive(Debug, Clone)]
pub struct Planned {
    pub reference: String,
    pub result: Result<InstallPlan, String>,
}

/// A fresh shelf and machine facts.
#[derive(Debug, Clone)]
pub struct Refreshed {
    /// Refresh order, so a slow older read never overwrites a newer one.
    pub sequence: u64,
    pub records: Vec<ModelRecord>,
    pub facts: Facts,
}

/// A key press, reduced to what the app distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Top,
    Bottom,
    Enter,
    Escape,
    Backspace,
    /// A printable character.
    Char(char),
    /// Ctrl-C.
    Interrupt,
}

/// How long the input thread blocks per poll before checking again, so a
/// closed receiver ends the thread promptly.
const POLL: Duration = Duration::from_millis(100);

/// Read terminal events on a blocking thread and forward them to `tx` until
/// the receiver goes away. Dropping `tx` when the thread ends is how the loop
/// learns that input is gone.
pub fn spawn_input(tx: mpsc::UnboundedSender<Event>) {
    thread::spawn(move || {
        loop {
            match event::poll(POLL) {
                Ok(true) => {}
                Ok(false) if tx.is_closed() => return,
                Ok(false) => continue,
                Err(_) => return,
            }
            let forwarded = match event::read() {
                Ok(event::Event::Key(key)) => translate(key).map(Event::Key),
                Ok(event::Event::Resize(_, _)) => Some(Event::Resize),
                Ok(_) => None,
                Err(_) => return,
            };
            if let Some(event) = forwarded
                && tx.send(event).is_err()
            {
                return;
            }
        }
    });
}

fn translate(key: KeyEvent) -> Option<Key> {
    if !key.kind.is_press() {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => Key::Interrupt,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Top,
        KeyCode::End => Key::Bottom,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) if !ctrl => Key::Char(c),
        _ => return None,
    })
}
