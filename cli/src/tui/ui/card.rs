//! The card every pane is drawn in: a rounded border on a surface a step up
//! from the ground, a title on its top edge at the left (`╭─ shelf · 15 ─`)
//! and a quieter label at the right (`─ by name ─╮`). On a card too narrow
//! for both, the title keeps the room and the label goes.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};
use unicode_width::UnicodeWidthStr;

use super::{DIM, INK, SURFACE};
use crate::tui::text;

/// Columns of air between a card's border and the text inside it.
const CARD_PAD: u16 = 1;

/// A card: its title, its right label, and how its border and ground read.
pub(super) struct Card {
    title: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    border: Style,
    ground: Color,
}

impl Card {
    /// A card titled with `title`, its border in the quiet register.
    pub(super) fn new(title: Vec<Span<'static>>) -> Self {
        Self {
            title,
            right: Vec::new(),
            border: Style::new().fg(super::LINE),
            ground: SURFACE,
        }
    }

    /// The same card with `right` as the label on its top edge's right.
    pub(super) fn right(mut self, right: Vec<Span<'static>>) -> Self {
        self.right = right;
        self
    }

    /// The same card with its border in `style`.
    pub(super) fn border(mut self, style: Style) -> Self {
        self.border = style;
        self
    }

    /// The rect inside the border, where a table goes: its first column is
    /// the selection gutter.
    pub(super) fn inner(area: Rect) -> Rect {
        Block::bordered().inner(area)
    }

    /// The rect inside the border and a column of air on each side, where
    /// text goes.
    pub(super) fn text_inner(area: Rect) -> Rect {
        let inner = Self::inner(area);
        Rect {
            x: inner.x.saturating_add(CARD_PAD),
            width: inner.width.saturating_sub(2 * CARD_PAD),
            ..inner
        }
    }

    /// Draw the card into `area` of `buf`.
    pub(super) fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.border)
            .style(Style::new().bg(self.ground))
            .render(area, buf);
        // Titles run between the corner's rule and the other corner's, each
        // padded with a space, so the edge reads `╭─ title ─── label ─╮`.
        let room = area.width.saturating_sub(4) as usize;
        let title_width = spans_width(&self.title);
        let right_width = spans_width(&self.right);
        // A title is drawn over the border, so a span with no colour of its
        // own would take the border's; it takes ink instead.
        let pad = |spans: Vec<Span<'static>>| {
            let mut padded = vec![Span::raw(" ")];
            padded.extend(
                spans
                    .into_iter()
                    .map(|span| Span::styled(span.content, INK.patch(span.style))),
            );
            padded.push(Span::raw(" "));
            padded
        };
        let both = title_width + 2 + 1 + right_width + 2 <= room;
        if title_width > 0 {
            let line = clipped(pad(self.title), room);
            let width = line.width() as u16;
            line.render(Rect::new(area.x + 2, area.y, width, 1), buf);
        }
        if right_width > 0 && (both || title_width == 0) {
            let line = clipped(pad(self.right), room);
            let width = line.width() as u16;
            let x = area.x + area.width - 2 - width;
            line.render(Rect::new(x, area.y, width, 1), buf);
        }
    }
}

/// The card's mark that a list scrolls: a heavy cell on its right border,
/// placed by how far through `total` rows the first of `visible` shown is.
pub(super) fn scroll_mark(
    buf: &mut Buffer,
    area: Rect,
    first: usize,
    visible: usize,
    total: usize,
) {
    let inner = Card::inner(area);
    if total <= visible || visible == 0 || inner.height == 0 || area.width == 0 {
        return;
    }
    let travel = inner.height.saturating_sub(1) as usize;
    let offset = (first * travel) / (total - visible).max(1);
    let y = inner.y + offset.min(travel) as u16;
    let x = area.x + area.width - 1;
    buf[(x, y)].set_symbol("┃").set_style(DIM);
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

/// `spans` held to `width` cells, the span it ends in clipped with `…`.
fn clipped(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut kept = Vec::new();
    let mut used = 0;
    for span in spans {
        let span_width = span.content.width();
        if used + span_width <= width {
            used += span_width;
            kept.push(span);
            continue;
        }
        let cut = text::clip(&span.content, width - used);
        if !cut.is_empty() {
            kept.push(Span::styled(cut, span.style));
        }
        break;
    }
    Line::from(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(card: Card, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        card.render(area, &mut buffer);
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn a_card_has_round_corners_a_title_and_a_label() {
        let card = Card::new(vec![Span::raw("shelf")]).right(vec![Span::raw("by name")]);
        let rows = rows(card, 30, 3);
        assert_eq!(rows[0], format!("╭─ shelf {} by name ─╮", "─".repeat(10)));
        assert_eq!(rows[1], format!("│{}│", " ".repeat(28)));
        assert_eq!(rows[2], format!("╰{}╯", "─".repeat(28)));
    }

    #[test]
    fn a_narrow_card_keeps_its_title_and_drops_the_label() {
        let card = Card::new(vec![Span::raw("shelf · 15")]).right(vec![Span::raw("by name")]);
        let rows = rows(card, 20, 3);
        assert_eq!(rows[0], "╭─ shelf · 15 ─────╮");
        let cramped = Card::new(vec![Span::raw("a very long title indeed")]);
        let rows = self::rows(cramped, 16, 3);
        assert_eq!(rows[0].chars().count(), 16);
        assert!(
            rows[0].ends_with("…─╮") || rows[0].ends_with("… ─╮"),
            "{}",
            rows[0]
        );
    }

    #[test]
    fn a_title_without_a_colour_of_its_own_reads_in_ink() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buffer = Buffer::empty(area);
        Card::new(vec![Span::styled("shelf", super::super::BOLD)]).render(area, &mut buffer);
        assert_eq!(buffer[(3, 0)].fg, crate::tui::palette::INK_COLOR);
    }

    #[test]
    fn the_ground_and_the_insets() {
        let area = Rect::new(0, 0, 20, 5);
        let mut buffer = Buffer::empty(area);
        Card::new(Vec::new()).render(area, &mut buffer);
        assert_eq!(buffer[(5, 2)].bg, SURFACE);
        assert_eq!(Card::inner(area), Rect::new(1, 1, 18, 3));
        assert_eq!(Card::text_inner(area), Rect::new(2, 1, 16, 3));
    }

    #[test]
    fn the_scroll_mark_travels_the_right_border() {
        let area = Rect::new(0, 0, 10, 12);
        let mut buffer = Buffer::empty(area);
        scroll_mark(&mut buffer, area, 0, 10, 20);
        assert_eq!(buffer[(9, 1)].symbol(), "┃");
        let mut buffer = Buffer::empty(area);
        scroll_mark(&mut buffer, area, 10, 10, 20);
        assert_eq!(buffer[(9, 10)].symbol(), "┃");
        let mut buffer = Buffer::empty(area);
        scroll_mark(&mut buffer, area, 0, 10, 10);
        assert!((0..12).all(|y| buffer[(9, y)].symbol() != "┃"));
    }
}
