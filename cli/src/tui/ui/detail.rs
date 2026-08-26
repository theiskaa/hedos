//! The detail pane for the selected model. Phase 1 draws the frame only.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;

use super::{BOLD, DIM};
use crate::tui::app::App;

/// Draw the detail frame into `area`, titled with the selected model.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.selected_record() {
        Some(record) => Line::from(Span::styled(format!(" {} ", record.display_name()), BOLD)),
        None => Line::from(Span::styled(" detail ", DIM)),
    };
    frame.render_widget(Block::bordered().title(title).border_style(DIM), area);
}
