//! The key line: each key dim, its verb plain; a notice takes the line over
//! while it lasts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, keys};
use crate::tui::app::App;

/// The keys the footer teaches, in display order.
const KEYS: [(&str, &str); 12] = [
    ("j/k", "move"),
    ("/", "filter"),
    ("p", "pull"),
    ("s", "scan"),
    ("w", "warm"),
    ("u", "unload"),
    ("l", "launch"),
    ("t", "try"),
    ("S", "serve"),
    ("x", "remove"),
    ("?", "help"),
    ("q", "quit"),
];

/// Draw the key line, or the current notice, into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let line = match app.notice() {
        Some(notice) => Line::from(Span::styled(format!(" {notice}"), BOLD)),
        None => keys(&KEYS),
    };
    frame.render_widget(Paragraph::new(line), area);
}
