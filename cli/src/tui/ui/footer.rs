//! The key line: each key dim, its verb plain; a notice takes the line over
//! while it lasts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM};
use crate::tui::app::App;

/// The keys the footer teaches, in display order.
const KEYS: [(&str, &str); 7] = [
    ("j/k", "move"),
    ("g/G", "ends"),
    ("s", "scan"),
    ("w", "warm"),
    ("u", "unload"),
    ("r", "refresh"),
    ("q", "quit"),
];

/// Draw the key line, or the current notice, into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let line = match app.notice() {
        Some(notice) => Line::from(Span::styled(format!(" {notice}"), BOLD)),
        None => keys(),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn keys() -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, verb) in KEYS {
        spans.push(Span::styled(key, DIM));
        spans.push(Span::raw(format!(" {verb}  ")));
    }
    Line::from(spans)
}
