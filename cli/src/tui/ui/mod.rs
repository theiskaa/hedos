//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's scroll position.

mod detail;
mod footer;
mod header;
mod shelf;

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use super::app::App;
use super::layout::Panes;

/// The quiet register: borders, labels, keys, models that can't run here.
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The loud register: the wordmark, warm models, the selected title.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

/// A `label   value` pair, the label dim and padded to `width`.
fn field<'a>(label: &'a str, value: impl Into<String>, width: usize) -> Vec<Span<'a>> {
    vec![
        Span::styled(format!(" {label:<width$}"), DIM),
        Span::raw(value.into()),
    ]
}

/// Draw one frame of `app`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let panes = Panes::compute(frame.area());
    header::draw(frame, panes.header, app);
    shelf::draw(frame, panes.shelf, app);
    detail::draw(frame, panes.detail, app);
    footer::draw(frame, panes.footer);
}
