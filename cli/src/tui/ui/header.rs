//! The one-line header: the wordmark and the shelf in numbers.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM};
use crate::tui::app::App;

/// Draw the header line into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let summary = format!("{} models · {} warm", app.records.len(), app.warm_count());
    let line = Line::from(vec![
        Span::styled(" hedos", BOLD),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
        Span::raw("  "),
        Span::styled(summary, DIM),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
