//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's scroll position.

mod detail;
mod footer;
mod header;
mod machine;
mod modal;
mod shelf;
mod tasks;

use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::App;
use super::layout::Panes;

/// The quiet register: borders, labels, keys, models that can't run here.
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The loud register: the wordmark, warm models, the selected title.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// The one colour on the screen: a muted warm tone for a task that failed.
const FAILED: Style = Style::new().fg(ratatui::style::Color::Rgb(201, 138, 106));

/// A `label   value` pair, the label dim and padded to `width`.
fn field<'a>(label: &'a str, value: impl Into<String>, width: usize) -> Vec<Span<'a>> {
    styled_field(label, value, width, Style::new())
}

/// [`field`] with the value in `style`; dim for a value that is an absence.
fn styled_field<'a>(
    label: &'a str,
    value: impl Into<String>,
    width: usize,
    style: Style,
) -> Vec<Span<'a>> {
    vec![
        Span::styled(format!(" {label:<width$}"), DIM),
        Span::styled(value.into(), style),
    ]
}

/// A key line: each key dim, its verb plain.
fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (key, verb) in pairs {
        spans.push(Span::styled((*key).to_owned(), DIM));
        spans.push(Span::raw(format!(" {verb}  ")));
    }
    Line::from(spans)
}

/// Draw one frame of `app`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let panes = Panes::compute(frame.area(), app.order.len(), app.tasks.len(), app.expanded);
    header::draw(frame, panes.header, app);
    if !app.expanded {
        shelf::draw(frame, panes.shelf, app);
        machine::draw(frame, panes.machine, panes.gateway, app);
    }
    detail::draw(frame, panes.detail, app);
    tasks::draw(frame, panes.tasks, app);
    footer::draw(frame, panes.footer, app);
    modal::draw(frame, frame.area(), app);
}
