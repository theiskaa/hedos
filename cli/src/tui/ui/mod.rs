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
//! grammar (`key_spans`, `keys`); the frames (`card`, `section`,
//! `selected_row`); `centered`; `spinner`. Every pane
//! and card imports only these and the state modules under `tui`, never
//! another pane.
//!
//! The wording register: lowercase, no sentence-final periods, `·` between
//! facts, keys as `key verb` with the verb from the keymap, a card's own
//! keys named where the card is drawn, and `;` joining two clauses in a
//! notice.

mod bench;
mod card;
mod chat;
mod detail;
mod footer;
mod header;
mod machine;
mod modal;
mod pull;
mod pulls;
mod shelf;
mod tasks;

pub(crate) use header::FIGURES_MS;
pub(crate) use machine::SEGMENT_MS;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::app::{App, Screen};
use super::edit::LineEdit;
use super::layout::{Panes, stacks};
use super::palette::{
    ACCENT, ACCENT_MID, BACKDROP, BAR_EMPTY, BAR_FILLED, BOLD, BORDER_COLUMNS, BORDER_ROWS,
    CAUTION, CURSOR, DIM, Depth, EYEBROW, FAILED, GROUND, INK, LINE, PAPER, SEGMENT_3,
    SELECTED_MARK, SELECTED_ROW, SOFT, TRACK, WARM, mix, onto_ground, quantize, spinner,
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

/// The wordmark: `hedos` bold, the version dim.
fn wordmark() -> [Span<'static>; 2] {
    [
        Span::styled(" hedos", BOLD),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
    ]
}

/// A section's heading over a run of rows: ` MEMORY ───────`, the rule
/// running to `width`.
fn section(name: &str, width: usize) -> Line<'static> {
    let head = format!(" {name} ");
    let rule = width.saturating_sub(head.width());
    Line::from(vec![
        Span::styled(head, EYEBROW),
        Span::styled("─".repeat(rule), Style::new().fg(LINE)),
    ])
}

/// A card titled with the pane's `name`, bold, for a pane that says no more
/// than what it is.
fn card(name: &str) -> card::Card {
    card::Card::new(vec![Span::styled(name.trim().to_owned(), BOLD)])
}

/// `line` as the selected row of a card `width` cells wide: the gutter
/// marked in its leading space, the tint patched under every span and
/// padded out to the card's edge, so the bar is one piece however short
/// the text.
fn selected_row(mut line: Line<'static>, width: usize) -> Line<'static> {
    if let Some(first) = line.spans.first_mut()
        && let Some(rest) = first.content.strip_prefix(' ')
    {
        let style = first.style;
        let rest = rest.to_owned();
        line.spans.remove(0);
        line.spans.insert(0, Span::styled(rest, style));
        line.spans.insert(0, Span::styled(SELECTED_MARK, ACCENT));
    }
    let pad = width.saturating_sub(line.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    line.patch_style(SELECTED_ROW)
}

/// `pairs` as spans: each key bright and bold, its verb quiet, two spaces
/// after, so the eye finds the letter to press first. Takes
/// the keymap's [`Pair`](super::keymap::Pair)s and the pairs a pane
/// phrases on the spot alike.
fn key_spans(pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    pairs
        .iter()
        .flat_map(|(key, verb)| {
            [
                Span::styled((*key).to_owned(), BOLD),
                Span::styled(format!(" {verb}  "), DIM),
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

/// Draw one frame of `app`: the ground first, every pane over it, then the
/// whole frame handed to the terminal's own ground and mapped onto its
/// palette when it has no more than 256 colours.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.note_area(area);
    frame.buffer_mut().set_style(area, GROUND);
    let panes = match app.screen {
        _ if app.pull_screen().is_some() => draw_pull(frame, app),
        Screen::Shelf => draw_shelf(frame, app),
        Screen::Pulls => draw_pulls(frame, app),
        Screen::Bench => draw_bench(frame, app),
    };
    tasks::draw(frame, panes.tasks, app);
    footer::draw(frame, panes.footer, app);
    fade_in_body(frame.buffer_mut(), panes.header, &app.motion);
    if modal::draw(frame, frame.area(), app) && app.notice().is_some() {
        // The backdrop flattens the footer with the rest of the screen, and
        // a notice raised from inside a card has to read, so its row is
        // painted again over the backdrop.
        frame.buffer_mut().set_style(panes.footer, Style::reset());
        frame.buffer_mut().set_style(panes.footer, GROUND);
        footer::draw(frame, panes.footer, app);
    }
    onto_ground(frame.buffer_mut(), app.ground);
    if app.depth == Depth::Indexed {
        quantize(frame.buffer_mut());
    }
}

/// When the body starts to come up at launch, and how long it takes.
const BODY_FROM_MS: u64 = 0;
const BODY_MS: u64 = 160;

/// Everything under the header raised from the ground at launch, quickly,
/// while the header arrives.
fn fade_in_body(
    buf: &mut ratatui::buffer::Buffer,
    header: Rect,
    motion: &crate::tui::motion::Motion,
) {
    let shown = motion.eased(BODY_FROM_MS, BODY_MS);
    if shown >= 1.0 {
        return;
    }
    let area = buf.area;
    let top = header.bottom();
    for y in top..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = mix(PAPER, cell.fg, shown);
            cell.bg = mix(PAPER, cell.bg, shown);
        }
    }
}

/// The shelf's body: header, shelf, machine block and detail, or the chat
/// pane in the body's place.
fn draw_shelf(frame: &mut Frame, app: &mut App) -> Panes {
    let stacked = stacks(frame.area());
    let chatting = app.chat_pane().is_some();
    // The try screen is the conversation alone: the task strip waits for the
    // shelf, and a notice still reaches the footer.
    let panes = Panes::compute(
        frame.area(),
        app.order.len(),
        machine::lines(stacked),
        if chatting { 0 } else { app.tasks.rows().len() },
        app.expanded || chatting,
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

/// The pull screen in the body, whichever screen it was opened from: the
/// header, then the search and its results and preview. The task strip
/// waits, since the screen shows the downloads itself.
fn draw_pull(frame: &mut Frame, app: &mut App) -> Panes {
    let panes = Panes::compute(frame.area(), 0, 0, 0, true);
    header::draw(frame, panes.header, app, false);
    pull::draw(frame, panes.detail, app);
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
        assert_eq!(spinner(SPINNER.len() as u64 + 1), SPINNER[1]);
    }

    #[test]
    fn keys_are_bright_and_their_verbs_quiet() {
        let spans = key_spans(&[("p", "pull")]);
        assert_eq!(spans[0].style, BOLD);
        assert_eq!(spans[1].style, DIM);
        assert_eq!(text(&Line::from(spans)), "p pull  ");
    }

    #[test]
    fn the_terminal_keeps_its_own_ground_under_every_cell() {
        use crate::tui::palette::{Ground, INK_COLOR, PAPER, RAISED, SEL};
        use ratatui::style::Color;
        let mut app = App::new(vec![record("m")], Facts::default());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("a test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        assert_eq!(
            terminal.backend().buffer()[(99, 29)].bg,
            PAPER,
            "a terminal that never said what its ground is gets the painted one"
        );
        app.ground = Ground::Terminal {
            light: false,
            rgb: None,
        };
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(99, 29)].bg, Color::Reset);
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| matches!(cell.bg, Color::Reset | SEL | RAISED)),
            "only the selected row and the chips keep a fill"
        );
        app.depth = Depth::Indexed;
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        assert_eq!(terminal.backend().buffer()[(99, 29)].bg, Color::Reset);
        app.ground = Ground::Terminal {
            light: true,
            rgb: None,
        };
        app.depth = Depth::True;
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(99, 29)].bg, Color::Reset);
        assert!(
            buffer.content.iter().all(|cell| cell.fg != INK_COLOR),
            "a light ground has no pale ink"
        );
    }

    /// Every screen and card, drawn whole at every size worth checking,
    /// settled and with the clock live mid-launch, never panics.
    #[test]
    fn every_screen_draws_at_every_size() {
        use crate::tui::motion::Motion;
        let sizes = [
            (1, 1),
            (20, 5),
            (40, 3),
            (80, 24),
            (95, 40),
            (96, 40),
            (99, 30),
            (100, 30),
            (120, 35),
            (132, 39),
            (132, 42),
            (200, 60),
        ];
        let opens: [&[Key]; 6] = [
            &[],
            &[Key::Char('t')],
            &[Key::Char('p')],
            &[Key::Char('?')],
            &[Key::Char('x')],
            &[Key::Enter],
        ];
        for live in [false, true] {
            for open in opens {
                for (width, height) in sizes {
                    let mut records: Vec<_> =
                        (0..20).map(|index| record(&format!("m{index}"))).collect();
                    records[3].state = kernel::records::ModelState::Missing;
                    let mut app = App::new(records, Facts::default());
                    if live {
                        app.motion = Motion::from_env_value_for_tests(None);
                        app.set_clock(700);
                    }
                    for key in open {
                        app.reduce(Event::Key(*key));
                    }
                    let mut terminal =
                        Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
                    terminal
                        .draw(|frame| draw(frame, &mut app))
                        .expect("a frame");
                }
            }
        }
    }

    #[test]
    fn a_notice_reads_over_the_backdrop() {
        let mut app = App::new(vec![record("m")], Facts::default());
        app.reduce(Event::Key(Key::Char('y')));
        assert_eq!(app.notice(), Some("m has no path"));
        app.reduce(Event::Key(Key::Char('?')));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("a test terminal");
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        let footer = 39;
        let notice: String = (0..buffer.area.width)
            .map(|x| buffer[(x, footer)].symbol())
            .collect();
        assert!(notice.starts_with(" › m has no path"), "{notice:?}");
        assert_ne!(
            buffer[(3, footer)].fg,
            BACKDROP.fg.expect("the backdrop's grey")
        );
        assert!(
            buffer[(3, footer)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert_eq!(buffer[(1, 0)].fg, BACKDROP.fg.expect("the backdrop's grey"));
    }
}
