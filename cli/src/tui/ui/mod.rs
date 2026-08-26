//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's scroll position. Colour is used
//! sparingly: an orange accent for what is in focus or names a mode, and the
//! terminal's own green for what is loaded, yellow for a tight fit, red for
//! what failed or won't fit.

mod detail;
mod footer;
mod header;
mod machine;
mod modal;
mod shelf;
mod tasks;

use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::App;
use super::layout::Panes;

/// The quiet register: borders, labels, keys, models that can't run here.
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The loud register: the wordmark, warm models, the selected title.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// What is in focus or names a mode: titles, eyebrows, the expanded detail.
/// A fixed orange, since no terminal palette has one and it should not drift
/// into the warning yellow.
const ACCENT: Style = Style::new().fg(Color::Rgb(232, 142, 68));
/// What is loaded.
const WARM: Style = Style::new().fg(Color::Green);
/// What only just fits.
const CAUTION: Style = Style::new().fg(Color::Yellow);
/// What failed or won't fit.
const FAILED: Style = Style::new().fg(Color::Red);
/// The glyphs of a horizontal bar: filled, then empty.
const BAR_FILLED: &str = "█";
const BAR_EMPTY: &str = "░";
/// The text cursor shown while something is being typed.
const CURSOR: &str = "▏";

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

/// A `label   value` line.
fn field_line(label: &str, value: impl Into<String>, width: usize) -> Line<'static> {
    Line::from(
        styled_field(label, value, width, Style::new())
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect::<Vec<_>>(),
    )
}

/// The wordmark: `hedos` bold, the version dim.
fn wordmark() -> [Span<'static>; 2] {
    [
        Span::styled(" hedos", BOLD),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
    ]
}

/// A bar of `width` cells, `filled` of them lit in `style`.
fn bar(filled: usize, width: usize, style: Style) -> [Span<'static>; 2] {
    let filled = filled.min(width);
    [
        Span::styled(BAR_FILLED.repeat(filled), style),
        Span::styled(BAR_EMPTY.repeat(width - filled), DIM),
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
    let panes = Panes::compute(
        frame.area(),
        app.order.len(),
        machine::lines(&app.facts),
        app.tasks.len(),
        app.expanded,
    );
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
