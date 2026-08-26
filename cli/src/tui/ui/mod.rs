//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's scroll position and the chat
//! pane's measure of how far its transcript scrolls. Colour is used
//! sparingly: an orange accent for what is in focus or names a mode, and the
//! terminal's own green for what is loaded, yellow for a tight fit, red for
//! what failed or won't fit.

mod chat;
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
use unicode_width::UnicodeWidthStr;

use super::app::App;
use super::edit::LineEdit;
use super::layout::Panes;

/// The quiet register: borders, labels, keys, models that can't run here.
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
/// The loud register: what the eye should land on first, from the wordmark
/// and warm models to the user's own words in the chat pane.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// What is in focus, names a mode, or is in motion: titles, eyebrows, the
/// expanded detail, a running task, the spinner. A fixed orange, since no
/// terminal palette has one and it should not drift into the warning yellow.
const ACCENT: Style = Style::new().fg(Color::Rgb(232, 142, 68));
/// What is loaded.
const WARM: Style = Style::new().fg(Color::Green);
/// A warning: a tight fit, a reply that was stopped.
const CAUTION: Style = Style::new().fg(Color::Yellow);
/// The selected row of a list.
const SELECTED_ROW: Style = Style::new().add_modifier(Modifier::REVERSED);
/// What failed or won't fit.
const FAILED: Style = Style::new().fg(Color::Red);
/// The glyphs of a horizontal bar: filled, then empty.
const BAR_FILLED: &str = "█";
const BAR_EMPTY: &str = "░";
/// The text cursor shown while something is being typed.
const CURSOR: &str = "▏";

/// A dim `label`, padded to `width`, in front of whatever a row shows.
fn label(label: &str, width: usize) -> Span<'static> {
    Span::styled(format!(" {label:<width$}"), DIM)
}

/// A `label   value` pair, the label dim and padded to `width`, the value
/// in `style`; dim for a value that is an absence.
fn styled_field(
    label: &str,
    value: impl Into<String>,
    width: usize,
    style: Style,
) -> Vec<Span<'static>> {
    vec![self::label(label, width), Span::styled(value.into(), style)]
}

/// A `label   value` line.
fn field_line(label: &str, value: impl Into<String>, width: usize) -> Line<'static> {
    Line::from(styled_field(label, value, width, Style::new()))
}

/// `mark` in `mark_style`, then `input` around its cursor, windowed so that
/// mark, text and cursor together take at most `width` cells.
fn edited(input: &LineEdit, mark: &str, mark_style: Style, width: usize) -> Vec<Span<'static>> {
    let (before, after) = input.view(width.saturating_sub(mark.width() + 1));
    vec![
        Span::styled(mark.to_owned(), mark_style),
        Span::raw(before),
        Span::styled(CURSOR, BOLD),
        Span::raw(after),
    ]
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

/// `pairs` as spans: each key dim, its verb plain, two spaces after.
fn key_spans(pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    pairs
        .iter()
        .flat_map(|(key, verb)| {
            [
                Span::styled((*key).to_owned(), DIM),
                Span::raw(format!(" {verb}  ")),
            ]
        })
        .collect()
}

/// A key line: a leading space, then [`key_spans`].
fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(key_spans(pairs));
    Line::from(spans)
}

/// Draw one frame of `app`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let panes = Panes::compute(
        frame.area(),
        app.order.len(),
        machine::lines(&app.facts),
        app.tasks.rows().len(),
        app.expanded || app.chat_pane().is_some(),
    );
    header::draw(frame, panes.header, app);
    if app.chat_pane().is_some() {
        chat::draw(frame, panes.detail, app);
    } else {
        if !app.expanded {
            shelf::draw(frame, panes.shelf, app);
            machine::draw(frame, panes.machine, panes.gateway, app);
        }
        detail::draw(frame, panes.detail, app);
    }
    tasks::draw(frame, panes.tasks, app);
    footer::draw(frame, panes.footer, app);
    modal::draw(frame, frame.area(), app);
}
