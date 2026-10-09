//! The try screen: a conversation with one model in the body. The
//! transcript card holds the turns (your words in a raised bubble on the
//! right, replies plain at reading width on the left), a rounded box at its
//! foot takes the next message, and beside them a session card keeps the
//! figures: residency, how much of the context is used, how fast the last
//! reply came. With nothing said yet, the transcript offers things to ask.
//! A judge gets a composer for typed questions in the box's place, and its
//! answers are drawn as distributions. An extractor gets one for the text and
//! what to do with it, and its answers are drawn as what was found.

mod composer;
mod draft;
mod empty;
mod extract_draft;
mod extraction;
mod judgment;
mod session;
mod transcript;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::card::{Card, scroll_mark};
use super::{BOLD, DIM, SOFT};
use crate::tui::app::App;
use crate::tui::chat::{ChatPane, Speaker, View};
use crate::tui::layout::GUTTER;
use crate::tui::motion::Motion;
use crate::tui::text;

/// The session card's width, and the narrowest body that keeps it.
const SESSION_WIDTH: u16 = 33;
const SESSION_FROM: u16 = 100;
/// How often a blinking cursor turns, in milliseconds.
const BLINK_MS: u64 = 500;

/// What the screen's parts need to move: the clock and the spinner's frame.
pub(super) struct Look<'a> {
    pub motion: &'a Motion,
    pub spin_frame: u64,
}

impl Look<'_> {
    /// Whether a blinking cursor shows now; always, when nothing moves.
    fn blink_on(&self) -> bool {
        self.motion
            .age(0)
            .is_none_or(|age| ((age * 1000.0) as u64 / BLINK_MS).is_multiple_of(2))
    }
}

/// Draw the try screen into `area`, the body under the header.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let spin_frame = app.spin_frame();
    let motion = app.motion;
    let warm = app
        .chat_pane()
        .is_some_and(|pane| app.facts.is_warm(&pane.record.id));
    let resident = app
        .chat_pane()
        .and_then(|pane| app.facts.resident(&pane.record.id).cloned());
    let collected_at = app.facts.collected_at_millis;
    let wide = frame.area().width >= SESSION_FROM;
    let Some(pane) = app.chat_pane_mut() else {
        return;
    };
    let look = Look {
        motion: &motion,
        spin_frame,
    };
    let session_width = if wide { SESSION_WIDTH + GUTTER } else { 0 };
    let left = Rect {
        width: area.width.saturating_sub(session_width),
        ..area
    };
    let composer_height = match (pane.draft.as_deref(), pane.extract.as_deref()) {
        (Some(draft), _) => draft::height(draft),
        (None, Some(draft)) => extract_draft::height(draft, left.width),
        (None, None) => composer::height(pane, left.width),
    }
    .min(area.height);
    let transcript = Rect {
        height: left.height.saturating_sub(composer_height),
        ..left
    };
    let composer = Rect {
        y: transcript.y + transcript.height,
        height: composer_height,
        ..left
    };
    draw_transcript(frame, transcript, pane, &look, warm, wide);
    match (pane.draft.as_deref(), pane.extract.as_deref()) {
        (Some(draft), _) => draft::draw(frame, composer, pane, draft, &look),
        (None, Some(draft)) => extract_draft::draw(frame, composer, pane, draft, &look),
        (None, None) => composer::draw(frame, composer, pane, &look),
    }
    if wide {
        let session = Rect {
            x: area.x + area.width - SESSION_WIDTH,
            width: SESSION_WIDTH,
            ..area
        };
        session::draw(
            frame,
            session,
            pane,
            &look,
            session::Residency {
                resident: resident.as_ref(),
                collected_at,
            },
        );
    }
}

/// The transcript card: its title names the model, its right label how far
/// the conversation has come, or where the view is held; the turns inside,
/// scrolled to the view.
fn draw_transcript(
    frame: &mut Frame,
    area: Rect,
    pane: &mut ChatPane,
    look: &Look,
    warm: bool,
    wide: bool,
) {
    let inner = Card::text_inner(area);
    let width = inner.width as usize;
    let lines = if pane.turns.is_empty() {
        Vec::new()
    } else {
        transcript::lines(pane, width, look)
    };
    let room = inner.height as usize;
    pane.measured(lines.len().saturating_sub(room));
    let first = pane.first_line();
    let turns = pane
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::User)
        .count();
    let mut right = match pane.view {
        View::Held(_) => vec![Span::styled(
            format!("line {} of {}", first + 1, lines.len()),
            SOFT,
        )],
        View::Follow if turns == 0 => vec![Span::styled("new", DIM)],
        View::Follow => vec![Span::styled(
            text::count(
                turns,
                if pane.judging() {
                    "ask"
                } else if pane.extracting() {
                    "read"
                } else {
                    "turn"
                },
            ),
            DIM,
        )],
    };
    if !wide {
        // Without the session card, its two most useful figures ride on the
        // transcript's edge.
        right.push(Span::styled(if warm { " · warm" } else { " · cold" }, DIM));
    }
    Card::new(vec![
        Span::styled(
            if pane.judging() {
                "judge "
            } else if pane.extracting() {
                "extract "
            } else {
                "try "
            },
            SOFT,
        ),
        Span::styled(
            crate::support::text::printable(pane.record.display_name()).into_owned(),
            BOLD,
        ),
    ])
    .right(right)
    .render(area, frame.buffer_mut());
    if pane.turns.is_empty() {
        empty::draw(frame, inner, pane, look);
        return;
    }
    let total = lines.len();
    let shown: Vec<_> = lines.into_iter().skip(first).take(room).collect();
    frame.render_widget(Paragraph::new(shown), inner);
    scroll_mark(frame.buffer_mut(), area, first, room, total);
    if let View::Held(_) = pane.view {
        let newer = total.saturating_sub(first + room);
        if newer > 0 {
            let note = format!(" ↓ {newer} newer lines ");
            let x = area.x + area.width.saturating_sub(note.chars().count() as u16) / 2;
            let y = area.y + area.height.saturating_sub(1);
            frame
                .buffer_mut()
                .set_stringn(x, y, &note, area.width as usize, SOFT);
        }
    }
}

/// `12:04`, the hour and minute `wall_ms` falls on in the machine's own
/// time zone; nothing before the loop has read the clock.
pub(super) fn clock_time(wall_ms: i64) -> Option<String> {
    if wall_ms <= 0 {
        return None;
    }
    let seconds = (wall_ms / 1000) as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: `localtime_r` reads the given time and writes a `tm` into the
    // buffer it is handed, returning null on failure, in which case the
    // buffer is never read.
    let filled = unsafe { libc::localtime_r(&seconds, local.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: only read once `localtime_r` reported that it filled it.
    let local = unsafe { local.assume_init() };
    Some(format!("{:02}:{:02}", local.tm_hour, local.tm_min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_clock_reads_as_hours_and_minutes() {
        assert_eq!(clock_time(0), None);
        let time = clock_time(1_700_000_000_000).expect("a time");
        assert_eq!(time.len(), 5);
        assert_eq!(&time[2..3], ":");
    }
}
