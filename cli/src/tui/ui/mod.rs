//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's scroll position.

mod detail;
mod footer;
mod header;
mod shelf;

use ratatui::Frame;
use ratatui::style::{Modifier, Style};

use super::app::App;
use super::layout::Panes;

/// The quiet register: borders, labels, keys, models that can't run here.
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The loud register: the wordmark, warm models, the selected title.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);

/// Draw one frame of `app`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let panes = Panes::compute(frame.area());
    header::draw(frame, panes.header, app);
    shelf::draw(frame, panes.shelf, app);
    detail::draw(frame, panes.detail, app);
    footer::draw(frame, panes.footer);
}
