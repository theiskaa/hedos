//! The pull screen: a search over the catalog and Hugging Face in the body,
//! the way `t` turns the body into a conversation. A field at the top takes
//! the query, chips under it pick a kind, the results run down the left
//! with a gauge of how each fits, the preview of the row under the cursor
//! sits on the right with the one button that pulls it, and the downloads
//! in flight stay in view under the results.

mod downloads;
mod preview;
mod results;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::card::Card;
use super::{BOLD, CURSOR, DIM, INK, SOFT, spinner};
use crate::tui::app::App;
use crate::tui::layout::GUTTER;
use crate::tui::motion::Motion;
use crate::tui::palette::{BOX_IDLE, BOX_LIVE, CHIP_ON};
use crate::tui::pull::{KINDS, PullModal, Search};
use crate::tui::text;

/// The preview's width beside the results, and the narrowest body that
/// keeps it there.
const PREVIEW_WIDTH: u16 = 45;
const PREVIEW_BESIDE_FROM: u16 = 100;
/// Rows of the downloads card, and the rows the results keep before it is
/// drawn.
const DOWNLOADS_ROWS: u16 = 6;
const RESULTS_FLOOR: u16 = 8;
/// Rows of the preview when it sits under the results.
const PREVIEW_STRIP_ROWS: u16 = 7;
/// What the field says while nothing is typed.
const PLACEHOLDER: &str = "search by name, owner/repo or name:tag";
/// How often a blinking cursor turns, in milliseconds.
const BLINK_MS: u64 = 500;

/// What the screen's parts need to move: the clock and the spinner's frame.
pub(super) struct Look<'a> {
    pub motion: &'a Motion,
    pub spin_frame: u64,
}

/// Draw the pull screen into `area`, the body under the header.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(modal) = app.pull_screen() else {
        return;
    };
    let look = Look {
        motion: &app.motion,
        spin_frame: app.spin_frame(),
    };
    let beside = frame.area().width >= PREVIEW_BESIDE_FROM;
    let left = Rect {
        width: if beside {
            area.width.saturating_sub(PREVIEW_WIDTH + GUTTER)
        } else {
            area.width
        },
        ..area
    };
    let search = Rect {
        height: 3.min(left.height),
        ..left
    };
    let chips = Rect {
        y: search.y + search.height,
        height: 1.min(left.height.saturating_sub(search.height)),
        ..left
    };
    let below = Rect {
        y: chips.y + chips.height,
        height: left.height.saturating_sub(search.height + chips.height),
        ..left
    };
    let downloading = app.tasks.pull_rows().next().is_some();
    let strip = if beside {
        0
    } else {
        PREVIEW_STRIP_ROWS.min(below.height)
    };
    let downloads =
        if (beside || downloading) && below.height >= RESULTS_FLOOR + DOWNLOADS_ROWS + strip {
            DOWNLOADS_ROWS
        } else {
            0
        };
    let results = Rect {
        height: below.height.saturating_sub(downloads + strip),
        ..below
    };
    let downloads_area = Rect {
        y: results.y + results.height,
        height: downloads,
        ..below
    };
    draw_search(frame, search, modal, &look);
    draw_chips(frame, chips, modal);
    results::draw(frame, results, modal, app, &look);
    if downloads > 0 {
        downloads::draw(frame, downloads_area, app, &look);
    }
    if beside {
        let preview = Rect {
            x: area.x + area.width - PREVIEW_WIDTH,
            width: PREVIEW_WIDTH,
            ..area
        };
        preview::draw(frame, preview, modal, app, &look, false);
    } else if strip > 0 {
        let preview = Rect {
            y: downloads_area.y + downloads_area.height,
            height: strip,
            ..below
        };
        preview::draw(frame, preview, modal, app, &look, true);
    }
}

/// The field: what was typed and its cursor, or the placeholder, and on
/// the right where each source of results is.
fn draw_search(frame: &mut Frame, area: Rect, modal: &PullModal, look: &Look) {
    let typing = !modal.input.is_empty();
    Card::new(Vec::new())
        .border(Style::new().fg(if typing { BOX_LIVE } else { BOX_IDLE }))
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let status = sources(modal.search(), look);
    let status_width: usize = status.iter().map(Span::width).sum();
    let room = (inner.width as usize).saturating_sub(status_width + 2);
    let blink = look
        .motion
        .age(0)
        .is_none_or(|age| ((age * 1000.0) as u64 / BLINK_MS).is_multiple_of(2));
    let mut spans = if typing {
        let (before, after) = modal.input.view(room.saturating_sub(1));
        vec![
            Span::styled(before, INK),
            Span::styled(if blink { CURSOR } else { " " }, BOLD),
            Span::styled(after, INK),
        ]
    } else {
        vec![
            Span::styled(if blink { CURSOR } else { " " }, BOLD),
            Span::styled(text::clip(PLACEHOLDER, room.saturating_sub(1)), DIM),
        ]
    };
    let used: usize = spans.iter().map(Span::width).sum();
    spans.push(Span::raw(" ".repeat(
        (inner.width as usize).saturating_sub(used + status_width),
    )));
    spans.extend(status);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// `✓ catalog   ⠋ hugging face`: the catalog is always there; Hugging Face
/// is searched once the query sits still, and can fail.
fn sources(search: &Search, look: &Look) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled("✓ ", SOFT),
        Span::styled("catalog", DIM),
        Span::raw("   "),
    ];
    match search {
        Search::Idle => spans.push(Span::styled("  hugging face", DIM)),
        Search::Due(_) | Search::Asked => {
            spans.push(Span::styled(format!("{} ", spinner(look.spin_frame)), INK));
            spans.push(Span::styled("hugging face", SOFT));
        }
        Search::Done => {
            spans.push(Span::styled("✓ ", SOFT));
            spans.push(Span::styled("hugging face", DIM));
        }
        Search::Failed(_) => spans.push(Span::styled("✕ hugging face", DIM)),
    }
    spans
}

/// The kinds as chips with their counts, the one shown raised; how many
/// results there are on the right.
fn draw_chips(frame: &mut Frame, area: Rect, modal: &PullModal) {
    if area.height == 0 {
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    for (index, kind) in KINDS.iter().enumerate() {
        let count = modal.count(*kind);
        if index == modal.kind {
            spans.push(Span::styled(
                format!(" {} {count} ", kind.label()),
                BOLD.bg(CHIP_ON),
            ));
        } else {
            spans.push(Span::styled(format!(" {}", kind.label()), SOFT));
            spans.push(Span::styled(format!(" {count} "), DIM));
        }
        spans.push(Span::raw(" "));
    }
    let results = text::count(modal.matches.len(), "result");
    let used: usize = spans.iter().map(Span::width).sum();
    let room = area.width as usize;
    if used + results.width() + 2 <= room {
        spans.push(Span::raw(" ".repeat(room - used - results.width() - 1)));
        spans.push(Span::styled(results, SOFT));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests;
