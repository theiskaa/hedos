//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's and the pulls list's scroll
//! positions and the chat pane's measure of how far its transcript scrolls.
//!
//! The style vocabulary, one meaning per style: `DIM` is the quiet register,
//! `BOLD` the loud one, `ACCENT` what is in focus, names a mode, or is in
//! motion, `EYEBROW` a heading over a run of rows and the name of a pane,
//! `COOL` where a model comes from and what runs it, and the three state
//! hues are `WARM` for what is loaded or up, `CAUTION` for a warning,
//! `FAILED` for what failed. `BACKDROP` flattens the screen behind a card
//! and `SELECTED_ROW` tints the selected row of a list. Every colour is a
//! fixed `Rgb` chosen against the orange accent, so the panes read as one
//! thing on any dark truecolor terminal instead of taking whatever the
//! palette's green and yellow happen to be; the machine's memory bar is
//! the one place the hues are swatches, not meanings.
//!
//! The shared helpers, in groups: measuring (`padded`, `right_aligned`,
//! `widest`); the label column (`label_width`, `value_width`, `label`,
//! `styled_field`, `field_line`); the one input (`edited`); the one key
//! grammar (`key_spans`, `keys`); the frames (`pane`, `selected_row`);
//! `bar`; `centered`; `spinner`. Every pane
//! and card imports only these and the state modules under `tui`, never
//! another pane.
//!
//! The wording register: lowercase, no sentence-final periods, `·` between
//! facts, keys as `key verb` with the verb from the keymap, a card's own
//! keys named where the card is drawn, and `;` joining two clauses in a
//! notice.

mod chat;
mod detail;
mod footer;
mod header;
mod machine;
mod modal;
mod pulls;
mod shelf;
mod tasks;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Screen};
use super::edit::LineEdit;
use super::layout::{Panes, stacks};
use super::text;

/// The three hues the panes are built from: the orange everything is
/// chosen against, a sand a step down from it, and its muted complement.
const ORANGE: Color = Color::Rgb(232, 142, 68);
const SAND: Color = Color::Rgb(198, 168, 128);
const TEAL: Color = Color::Rgb(112, 166, 162);
/// The quiet register: borders, labels, keys, models that can't run here.
/// A warm grey rather than the DIM modifier, which lands anywhere from
/// unreadable to plain white depending on the terminal.
const DIM: Style = Style::new().fg(Color::Rgb(124, 116, 106));
/// The loud register: what the eye should land on first, from the wordmark
/// and warm models to the user's own words in the chat pane.
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
/// What is in focus, names a mode, or is in motion: the expanded detail's
/// frame, an input mark, a running task's verb, the spinner, the download
/// bar, the chat's and the cards' titles. The wordmark is the one still
/// thing that wears it, being the orange the rest is built around.
const ACCENT: Style = Style::new().fg(ORANGE);
/// A heading over a run of rows, the name of a pane, and the koala beside
/// the wordmark: the shelf's column headers, the detail's MEMORY, the pull
/// listing's categories, the help's groups. The sand frames without
/// competing with what moves or has focus.
const EYEBROW: Style = Style::new().fg(SAND);
/// Where a model comes from and what runs it: the runtime and store
/// columns and rows. The teal reads as a fact and not a signal.
const COOL: Style = Style::new().fg(TEAL);
/// What is loaded or up: a warm model, a gateway that is on.
const WARM: Style = Style::new().fg(Color::Rgb(128, 196, 136));
/// A warning: a tight fit, a reply that was stopped.
const CAUTION: Style = Style::new().fg(Color::Rgb(230, 186, 96));
/// The selected row of a list: a warm dark tint under the row, so the
/// text keeps its hues where a reversed row would flatten them. Patched
/// over a row, never set, so a dim row stays dim under it.
const SELECTED_ROW: Style = Style::new().bg(Color::Rgb(58, 48, 40));
/// What failed, and nothing else.
const FAILED: Style = Style::new().fg(Color::Rgb(226, 108, 98));
/// The screen behind a card: every colour and emphasis flattened to one
/// near-black grey so the card is the only thing lit. A fixed value, since
/// palette greys land too bright on many terminals to read as a backdrop;
/// the selection's tint goes with the rest.
const BACKDROP: Style = Style::new()
    .fg(Color::Rgb(44, 44, 44))
    .bg(Color::Reset)
    .remove_modifier(Modifier::BOLD);
/// The gutter mark on the selected row of a list, in the cell its leading
/// space took; the one selection signal a terminal without truecolor keeps.
const SELECTED_MARK: &str = "▎";
/// Rows a bordered block spends on its top and bottom edges.
pub(super) const BORDER_ROWS: u16 = 2;
/// Columns a bordered block spends on its left and right edges.
pub(super) const BORDER_COLUMNS: u16 = 2;
/// The glyphs of a horizontal bar: filled, then empty.
const BAR_FILLED: &str = "█";
const BAR_EMPTY: &str = "░";
/// The text cursor shown while something is being typed.
const CURSOR: &str = "▏";
/// The glyphs of the spinner that turns while something is waited on, one
/// per tick.
const SPINNER: [&str; 6] = ["⠋", "⠙", "⠸", "⠴", "⠦", "⠇"];

/// The spinner's glyph on tick `ticks`.
fn spinner(ticks: u64) -> &'static str {
    SPINNER[(ticks % SPINNER.len() as u64) as usize]
}

/// `text` padded with spaces to `width` terminal cells; a wide glyph counts
/// for two, where `{:<width$}` would count it once and leave the column
/// ragged.
fn padded(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(pad))
}

/// `text` right-aligned in `width` cells, counted the same way.
fn right_aligned(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{}{text}", " ".repeat(pad))
}

/// Cells the widest of `texts` takes; none, nothing.
fn widest(texts: &[&str]) -> usize {
    texts.iter().map(|text| text.width()).max().unwrap_or(0)
}

/// The width of a label column: the widest of `labels`, then `gap` cells
/// before the value.
fn label_width(labels: &[&str], gap: usize) -> usize {
    widest(labels) + gap
}

/// Cells a labelled value may take in a pane `width` cells wide: what the
/// leading space, a label column `labels` wide, and a cell of air on the
/// right leave.
fn value_width(width: usize, labels: usize) -> usize {
    width.saturating_sub(labels + 2)
}

/// A dim `label`, padded to `width`, in front of whatever a row shows.
fn label(label: &str, width: usize) -> Span<'static> {
    Span::styled(format!(" {}", padded(label, width)), DIM)
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

/// `mark` in the accent, then `input` around its cursor, windowed so that
/// mark, text and cursor together take at most `width` cells; while nothing
/// is typed, a dim `placeholder` stands where the text will go.
fn edited(input: &LineEdit, mark: &str, width: usize, placeholder: &str) -> Vec<Span<'static>> {
    let room = width.saturating_sub(mark.width() + 1);
    if input.is_empty() {
        return vec![
            Span::styled(mark.to_owned(), ACCENT),
            Span::styled(text::clip(placeholder, room), DIM),
        ];
    }
    let (before, after) = input.view(room);
    vec![
        Span::styled(mark.to_owned(), ACCENT),
        Span::raw(before),
        Span::styled(CURSOR, BOLD),
        Span::raw(after),
    ]
}

/// The wordmark: `hedos` bold in the accent, the version dim.
fn wordmark() -> [Span<'static>; 2] {
    [
        Span::styled(" hedos", ACCENT.add_modifier(Modifier::BOLD)),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
    ]
}

/// A pane's frame: its `name` as an eyebrow over dim borders.
fn pane(name: &'static str) -> Block<'static> {
    Block::bordered()
        .title(Span::styled(name, EYEBROW))
        .border_style(DIM)
}

/// `line` as the selected row of a card `width` cells wide: the gutter
/// marked in its leading space, the tint patched under every span and
/// padded out to the card's edge, so the bar is one piece however short
/// the text.
fn selected_row(mut line: Line<'static>, width: usize) -> Line<'static> {
    if let Some(first) = line.spans.first_mut()
        && let Some(rest) = first.content.strip_prefix(' ')
    {
        first.content = format!("{SELECTED_MARK}{rest}").into();
    }
    let pad = width.saturating_sub(line.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    line.patch_style(SELECTED_ROW)
}

/// A bar of `width` cells, `filled` of them lit in `style`.
fn bar(filled: usize, width: usize, style: Style) -> [Span<'static>; 2] {
    let filled = filled.min(width);
    [
        Span::styled(BAR_FILLED.repeat(filled), style),
        Span::styled(BAR_EMPTY.repeat(width - filled), DIM),
    ]
}

/// `pairs` as spans: each key dim, its verb plain, two spaces after. Takes
/// the keymap's [`Pair`](super::keymap::Pair)s and the pairs a pane
/// phrases on the spot alike.
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
    let panes = match app.screen {
        Screen::Shelf => draw_shelf(frame, app),
        Screen::Pulls => draw_pulls(frame, app),
    };
    tasks::draw(frame, panes.tasks, app);
    footer::draw(frame, panes.footer, app);
    if modal::draw(frame, frame.area(), app) && app.notice().is_some() {
        // The backdrop flattens the footer with the rest of the screen, and
        // a notice raised from inside a card has to read, so its row is
        // painted again over the backdrop.
        frame.buffer_mut().set_style(panes.footer, Style::reset());
        footer::draw(frame, panes.footer, app);
    }
}

/// The shelf's body: header, shelf, machine block and detail, or the chat
/// pane in the body's place.
fn draw_shelf(frame: &mut Frame, app: &mut App) -> Panes {
    let stacked = stacks(frame.area());
    let panes = Panes::compute(
        frame.area(),
        app.order.len(),
        machine::lines(&app.facts, stacked),
        app.tasks.rows().len(),
        app.expanded || app.chat_pane().is_some(),
    );
    header::draw(frame, panes.header, app, panes.machine.height > 0);
    if app.chat_pane().is_some() {
        chat::draw(frame, panes.detail, app);
    } else {
        if !app.expanded {
            shelf::draw(frame, panes.shelf, app);
            machine::draw(frame, panes.machine, panes.gateway, app, stacked);
        }
        detail::draw(frame, panes.detail, app);
    }
    panes
}

/// The pulls screen's body: header, the list where the shelf goes, the
/// selected pull where the model's detail goes.
fn draw_pulls(frame: &mut Frame, app: &mut App) -> Panes {
    let panes = Panes::pulls(frame.area(), app.pulls.rows().len(), app.tasks.rows().len());
    header::draw(frame, panes.header, app, false);
    pulls::draw_list(frame, panes.shelf, app);
    pulls::draw_detail(frame, panes.detail, app);
    panes
}

/// A rect of `width` by `height` in the middle of `area`, no larger than
/// `area` itself.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height)),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, rect, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width.min(area.width)),
        Constraint::Fill(1),
    ])
    .areas(middle);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::event::{Event, Key};
    use crate::tui::facts::Facts;
    use crate::tui::testing::{record, text};

    #[test]
    fn padded_counts_cells_not_chars() {
        assert_eq!(padded("日本", 6), "日本  ");
        assert_eq!(padded("abc", 5), "abc  ");
        assert_eq!(padded("abcdef", 3), "abcdef");
    }

    #[test]
    fn right_aligned_counts_cells_not_chars() {
        assert_eq!(right_aligned("日本", 6), "  日本");
        assert_eq!(right_aligned("abcdef", 3), "abcdef");
    }

    #[test]
    fn a_label_column_is_the_widest_label_and_the_gap() {
        assert_eq!(widest(&["memory", "disk"]), 6);
        assert_eq!(widest(&["日本", "ab"]), 4);
        assert_eq!(widest(&[]), 0);
        assert_eq!(label_width(&["memory", "disk"], 1), 7);
        assert_eq!(label_width(&[], 2), 2);
        assert_eq!(value_width(80, 7), 71);
        assert_eq!(value_width(5, 7), 0);
    }

    #[test]
    fn an_empty_field_shows_its_placeholder_in_place_of_the_cursor() {
        let mut input = LineEdit::default();
        let blank = Line::from(edited(&input, " › ", 20, "name, owner/repo or name:tag"));
        assert_eq!(text(&blank), " › name, owner/rep…");
        assert!(blank.width() <= 20);
        assert!(!text(&blank).contains(CURSOR));
        assert_eq!(blank.spans[1].style, DIM);
        input.apply(Key::Char('q'));
        let typed = text(&Line::from(edited(&input, " › ", 20, "unused")));
        assert_eq!(typed, format!(" › q{CURSOR}"));
    }

    #[test]
    fn the_spinner_cycles_by_tick() {
        assert_eq!(spinner(0), SPINNER[0]);
        assert_eq!(spinner(7), SPINNER[1]);
    }

    #[test]
    fn a_notice_reads_over_the_backdrop() {
        let mut app = App::new(vec![record("m")], Facts::default());
        app.reduce(Event::Key(Key::Char('y')));
        assert_eq!(app.notice(), Some("m has no path"));
        app.reduce(Event::Key(Key::Char('p')));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("a test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        let footer = 39;
        let notice: String = (0..buffer.area.width)
            .map(|x| buffer[(x, footer)].symbol())
            .collect();
        assert!(notice.starts_with(" m has no path"), "{notice:?}");
        assert_ne!(
            buffer[(1, footer)].fg,
            BACKDROP.fg.expect("the backdrop's grey")
        );
        assert!(buffer[(1, footer)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(1, 0)].fg, BACKDROP.fg.expect("the backdrop's grey"));
    }
}
