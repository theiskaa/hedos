//! Everything that wakes the loop: keys from the terminal, the tick, progress
//! from background tasks, and a refreshed shelf. Keys are translated into the
//! app's own [`Key`] here so the reducer never sees terminal types.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::Duration;

use kernel::capabilities::GenerationStats;
use kernel::install::plan::{InstallPlan, InstallSearchHit};
use kernel::install::provider::InstallProviderId;
use kernel::records::ModelRecord;
use ratatui::crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
use runtime::bench::BenchEvent;
use tokio::sync::mpsc;

use super::facts::Facts;
use super::jobs::JobRow;
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
    /// The animation clock wants a new frame drawn.
    Frame,
    /// A background task moved.
    Task(TaskEvent),
    /// The pull jobs, as the job directory reads right now.
    Pulls(Vec<JobRow>),
    /// One job's history, as lines already phrased against the clock.
    History { job: String, lines: Vec<String> },
    /// A pull was started from here, as this job.
    PullStarted(String),
    /// A pull could not be started, stopped, or resumed, and why.
    PullRefused(String),
    /// The shelf and facts were re-read.
    Refreshed(Refreshed),
    /// A provider search came back.
    Searched(Searched),
    /// An install plan came back.
    Planned(Planned),
    /// The chat pane's reply moved.
    Reply(Reply),
    /// A step of the bench in flight, stamped with the bench it belongs to so
    /// a step from one already replaced cannot land on its successor.
    Bench { generation: u64, step: BenchEvent },
    /// A bench's driver returned, however it went.
    BenchEnded(u64),
    /// Text pasted into the terminal, whole, line breaks and all.
    Paste(String),
    /// The terminal stopped delivering keys; nothing can drive the UI now.
    InputClosed,
}

/// The hits for a query, or why there are none.
#[derive(Debug, Clone)]
pub struct Searched {
    pub query: String,
    pub hits: Vec<InstallSearchHit>,
    /// Why the search came back short, when a provider could not be asked.
    pub note: Option<String>,
}

/// The plan for a reference, or why it could not be made.
#[derive(Debug, Clone)]
pub struct Planned {
    /// Which ask this answers, so an answer to an older one never lands.
    pub ask: u64,
    /// What was planned.
    pub provider: InstallProviderId,
    pub reference: String,
    pub result: Result<InstallPlan, String>,
}

/// A step of a streamed reply, stamped with the ask it answers so a reply
/// that was stopped can't reach the turn that came after it.
#[derive(Debug, Clone)]
pub struct Reply {
    pub generation: u64,
    pub step: ReplyStep,
}

/// What a reply did.
#[derive(Debug, Clone)]
pub enum ReplyStep {
    /// More visible text.
    Text(String),
    /// The reply ended, with the runtime's stats if it reported any.
    Done(Option<GenerationStats>),
    /// The runtime gave up, with the reason.
    Failed(String),
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
    /// The wheel or trackpad, a notch at a time.
    ScrollUp,
    ScrollDown,
    /// A page up the chat transcript.
    PageUp,
    /// A page down it.
    PageDown,
    /// A line-editing key, meaningful only while typing into a field.
    Edit(Edit),
    /// Tab: the next suggestion, or the next kind.
    Tab,
    /// Shift-Tab: the previous one.
    BackTab,
    /// Ctrl-L: start over.
    Clear,
}

/// The line-editing keys, named the way a shell names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// One character back.
    Left,
    /// One character forward.
    Right,
    /// Option+Left, Alt-b.
    WordLeft,
    /// Option+Right, Alt-f.
    WordRight,
    /// Ctrl-A.
    Start,
    /// Ctrl-E.
    End,
    /// Forward delete.
    Delete,
    /// Ctrl-U: everything before the cursor. Cmd+Delete never reaches a
    /// terminal program; a terminal can be told to send this for it.
    KillToStart,
    /// Ctrl-W, Option+Delete: the word before the cursor.
    KillWord,
}

/// How long the input thread blocks per poll before checking again, so a
/// stop request is honoured promptly.
const POLL: Duration = Duration::from_millis(100);

/// How long handing the terminal over waits for the input thread to let go.
const LET_GO: Duration = Duration::from_secs(1);

/// The input thread. [`Input::hand_over`] stops the reader and waits for it to
/// let go of the terminal, so whatever takes the terminal next is its only
/// reader. Dropped instead, it is told to stop and left to end on its own:
/// the UI is on its way out, and a reader of a terminal that hung up never
/// ends, since crossterm reads its end of input over and over.
pub struct Input {
    stop: Arc<AtomicBool>,
    /// Disconnects when the thread ends.
    ended: std_mpsc::Receiver<()>,
}

impl Input {
    /// Read terminal events on a blocking thread and forward them to `tx`
    /// until stopped. A terminal that stops delivering keys is reported as
    /// [`Event::InputClosed`] so the loop can end instead of ticking on.
    pub fn spawn(tx: mpsc::UnboundedSender<Event>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let (ending, ended) = std_mpsc::channel::<()>();
        thread::spawn(move || {
            let _ending = ending;
            // Whatever crossterm parsed while the UI was away was typed at
            // something else.
            while matches!(event::poll(Duration::ZERO), Ok(true)) {
                let _ = event::read();
            }
            let mut replies = LateReplies::default();
            loop {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                match event::poll(POLL) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => {
                        let _ = tx.send(Event::InputClosed);
                        return;
                    }
                }
                let forwarded = match event::read() {
                    Ok(event::Event::Key(key)) if replies.swallows(&key) => None,
                    Ok(event::Event::Key(key)) => translate(key).map(Event::Key),
                    Ok(event::Event::Mouse(mouse)) => match mouse.kind {
                        MouseEventKind::ScrollUp => Some(Event::Key(Key::ScrollUp)),
                        MouseEventKind::ScrollDown => Some(Event::Key(Key::ScrollDown)),
                        _ => None,
                    },
                    Ok(event::Event::Resize(_, _)) => Some(Event::Resize),
                    Ok(event::Event::Paste(text)) => Some(Event::Paste(text)),
                    Ok(_) => None,
                    Err(_) => {
                        let _ = tx.send(Event::InputClosed);
                        return;
                    }
                };
                if let Some(event) = forwarded
                    && tx.send(event).is_err()
                {
                    return;
                }
            }
        });
        Self { stop, ended }
    }

    /// Stop the reader and wait, at most [`LET_GO`], for it to let go of the
    /// terminal. One still reading by then is stuck on a terminal that hung
    /// up, which nothing else will read either.
    pub fn hand_over(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.ended.recv_timeout(LET_GO);
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A terminal's answer to the colour query at launch that came too late to
/// be read as one. crossterm hands `ESC ] 11;rgb:2828/2c2c/3434 BEL` over as
/// Alt+`]`, the answer's characters as plain keys, and Ctrl+G (or Alt+`\`
/// for an `ESC \` ending); left in, those characters would press the
/// shelf's keys. An answer is taken from its Alt+`]` to its ending, or to
/// the first key no answer holds.
#[derive(Debug, Default)]
struct LateReplies {
    inside: bool,
}

impl LateReplies {
    /// Whether `key` is part of a late answer rather than a key pressed.
    fn swallows(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let answer_char = |c: char| c.is_ascii_hexdigit() || "rgb;:/?".contains(c);
        match key.code {
            KeyCode::Char(']') if alt => {
                self.inside = true;
                true
            }
            _ if !self.inside => false,
            KeyCode::Char('g') if ctrl => {
                self.inside = false;
                true
            }
            KeyCode::Char('\\') if alt => {
                self.inside = false;
                true
            }
            KeyCode::Char(c) if !ctrl && !alt && answer_char(c) => true,
            _ => {
                self.inside = false;
                false
            }
        }
    }
}

fn translate(key: KeyEvent) -> Option<Key> {
    if !key.kind.is_press() {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => Key::Interrupt,
        KeyCode::Char('l') if ctrl => Key::Clear,
        KeyCode::Char('a') if ctrl => Key::Edit(Edit::Start),
        KeyCode::Char('e') if ctrl => Key::Edit(Edit::End),
        KeyCode::Char('u') if ctrl => Key::Edit(Edit::KillToStart),
        KeyCode::Char('w') if ctrl => Key::Edit(Edit::KillWord),
        KeyCode::Char('b') if alt => Key::Edit(Edit::WordLeft),
        KeyCode::Char('f') if alt => Key::Edit(Edit::WordRight),
        KeyCode::Backspace if alt => Key::Edit(Edit::KillWord),
        KeyCode::Left if alt => Key::Edit(Edit::WordLeft),
        KeyCode::Right if alt => Key::Edit(Edit::WordRight),
        KeyCode::Left => Key::Edit(Edit::Left),
        KeyCode::Right => Key::Edit(Edit::Right),
        KeyCode::Delete => Key::Edit(Edit::Delete),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Top,
        KeyCode::End => Key::Bottom,
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) if !ctrl && !alt => Key::Char(c),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` as crossterm hands it over: each byte a key, `ESC x` an Alt+x,
    /// a control byte a Ctrl key.
    fn keys(text: &str) -> Vec<KeyEvent> {
        let mut keys = Vec::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            keys.push(match c {
                '\x1b' => KeyEvent::new(
                    KeyCode::Char(chars.next().expect("a key after ESC")),
                    KeyModifiers::ALT,
                ),
                '\x07' => KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
                c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            });
        }
        keys
    }

    /// What of `text` reaches the shelf as typed characters.
    fn passed(text: &str) -> String {
        let mut replies = LateReplies::default();
        keys(text)
            .into_iter()
            .filter(|key| !replies.swallows(key))
            .filter_map(|key| match key.code {
                KeyCode::Char(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_late_colour_answer_never_reaches_the_shelf_as_keys() {
        assert_eq!(passed("\x1b]11;rgb:2828/2c2c/3434\x07j"), "j");
        assert_eq!(
            passed("\x1b]10;rgb:abab/b2b2/bfbf\x1b\\\x1b]11;rgb:2828/2c2c/3434\x1b\\k"),
            "k"
        );
    }

    #[test]
    fn keys_outside_an_answer_pass() {
        assert_eq!(passed("bcdrg/"), "bcdrg/");
        assert_eq!(passed("\x1b]x b"), "x b", "a key no answer holds ends it");
    }
}
