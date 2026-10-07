//! The box the next message is typed into: rounded, its border brighter
//! while something is typed, growing to three lines before it scrolls, with
//! `enter send` inside on the right. While a reply streams it says so in its
//! border and offers `esc` to stop.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::Look;
use crate::tui::chat::ChatPane;
use crate::tui::palette::{BOX_IDLE, BOX_LIVE, LINE};
use crate::tui::ui::card::Card;
use crate::tui::ui::{ACCENT, BOLD, CURSOR, DIM, INK, SOFT, spinner};

/// The most lines the box shows before it scrolls.
const MAX_LINES: usize = 3;
/// The room the key on the right takes, with its gap.
const HINT_WIDTH: usize = 12;

/// How tall the box is for `pane` at `width` columns: its lines and its
/// border.
pub(super) fn height(pane: &ChatPane, width: u16) -> u16 {
    let room = text_width(width);
    let (lines, _) = pane.input.wrapped(room);
    lines.len().clamp(1, MAX_LINES) as u16 + 2
}

/// The cells a line of typing gets in a box `width` columns wide.
fn text_width(width: u16) -> usize {
    Card::text_inner(Rect::new(0, 0, width, 3))
        .width
        .saturating_sub(HINT_WIDTH as u16)
        .max(1) as usize
}

/// Draw the box into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, pane: &ChatPane, look: &Look) {
    let streaming = pane.streaming();
    let typing = !pane.input.is_empty();
    let border = if streaming {
        LINE
    } else if typing {
        BOX_LIVE
    } else {
        BOX_IDLE
    };
    let title = if streaming {
        vec![
            Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT),
            Span::styled("replying", SOFT),
        ]
    } else {
        Vec::new()
    };
    Card::new(title)
        .border(Style::new().fg(border))
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let room = text_width(area.width);
    let blink = look.blink_on();
    let mut lines: Vec<Line> = if typing {
        let (wrapped, (row, column)) = pane.input.wrapped(room);
        let first = (row + 1).saturating_sub(MAX_LINES);
        wrapped
            .into_iter()
            .enumerate()
            .skip(first)
            .take(MAX_LINES)
            .map(|(index, text)| {
                if index != row || !blink {
                    return Line::from(Span::styled(text, INK));
                }
                let split = text
                    .char_indices()
                    .scan(0, |cells, (offset, c)| {
                        let at = *cells;
                        *cells += c.to_string().width();
                        Some((offset, at))
                    })
                    .find(|(_, at)| *at >= column)
                    .map_or(text.len(), |(offset, _)| offset);
                let (before, after) = text.split_at(split);
                Line::from(vec![
                    Span::styled(before.to_owned(), INK),
                    Span::styled(CURSOR, BOLD),
                    Span::styled(after.to_owned(), INK),
                ])
            })
            .collect()
    } else {
        let name = crate::support::text::printable(pane.record.display_name()).into_owned();
        let placeholder = if streaming {
            "esc stops the reply".to_owned()
        } else if pane.turns.is_empty() {
            format!("ask {name} anything")
        } else {
            format!("ask {name} a follow-up")
        };
        let caret = if blink && !streaming { CURSOR } else { " " };
        vec![Line::from(vec![
            Span::styled(caret, BOLD),
            Span::styled(crate::tui::text::clip(&placeholder, room), DIM),
        ])]
    };
    let hint = if streaming {
        vec![Span::styled("esc", BOLD), Span::styled(" stop", DIM)]
    } else {
        vec![
            Span::styled("enter", if typing { BOLD } else { DIM }),
            Span::styled(" send", DIM),
        ]
    };
    if let Some(last) = lines.last_mut() {
        let used = last.width();
        let hint_width: usize = hint.iter().map(Span::width).sum();
        let pad = (inner.width as usize).saturating_sub(used + hint_width);
        last.spans.push(Span::raw(" ".repeat(pad)));
        last.spans.extend(hint);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::event::Key;
    use crate::tui::motion::Motion;
    use crate::tui::testing::record;

    fn drawn(pane: &ChatPane, width: u16) -> Vec<String> {
        let height = height(pane, width);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
        let motion = Motion::settled();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    pane,
                    &Look {
                        motion: &motion,
                        spin_frame: 0,
                    },
                )
            })
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn an_empty_box_asks_and_offers_enter() {
        let pane = ChatPane::open(record("qwen"));
        let rows = drawn(&pane, 60);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].contains("▏ask qwen anything"), "{:?}", rows[1]);
        assert!(
            rows[1].trim_end_matches(['│', ' ']).ends_with("enter send"),
            "{:?}",
            rows[1]
        );
    }

    #[test]
    fn the_box_grows_to_three_lines_then_scrolls() {
        let mut pane = ChatPane::open(record("m"));
        for c in "word ".repeat(30).chars() {
            pane.edit(Key::Char(c));
        }
        assert_eq!(height(&pane, 60), 5);
        let rows = drawn(&pane, 60);
        assert!(
            rows[3].contains('▏'),
            "the cursor is on the last line: {rows:?}"
        );
        let mut short = ChatPane::open(record("m"));
        short.edit(Key::Char('a'));
        assert_eq!(height(&short, 60), 3);
    }
}
