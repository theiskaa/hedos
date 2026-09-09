//! Drawing the app state. Panes read the app and write to the frame; the only
//! mutable state they touch is the shelf's and the pulls list's scroll
//! positions and the chat pane's measure of how far its transcript scrolls.
//!
//! The style vocabulary the panes draw with lives in [`super::palette`],
//! which the bench view shares.
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

mod bench;
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
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use unicode_width::UnicodeWidthStr;

use super::app::{App, Screen};
use super::edit::LineEdit;
use super::layout::{Panes, stacks};
use super::palette::{
    ACCENT, BACKDROP, BAR_EMPTY, BAR_FILLED, BOLD, BORDER_COLUMNS, BORDER_ROWS, CAUTION, COOL,
    CURSOR, DIM, EYEBROW, FAILED, ORANGE, SAND, SELECTED_MARK, SELECTED_ROW, TEAL, WARM, spinner,
};
use super::text;
use crate::support::text::{padded, right_aligned};

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
        Screen::Bench => draw_bench(frame, app),
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

/// The bench screen: the models being measured where the shelf goes, the
/// selected row's figures beside them.
fn draw_bench(frame: &mut Frame, app: &mut App) -> Panes {
    let panes = Panes::bench(frame.area(), app.bench.rows().len(), app.tasks.rows().len());
    header::draw(frame, panes.header, app, false);
    bench::draw_list(frame, panes.shelf, app);
    bench::draw_detail(frame, panes.detail, app);
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
    use crate::tui::palette::SPINNER;

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
