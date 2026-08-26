//! The key line: each key dim, its verb plain.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::DIM;

/// The keys the footer teaches, in display order.
const KEYS: [(&str, &str); 3] = [("j/k", "move"), ("g/G", "ends"), ("q", "quit")];

/// Draw the key line into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (key, verb) in KEYS {
        spans.push(Span::styled(key, DIM));
        spans.push(Span::raw(format!(" {verb}  ")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
