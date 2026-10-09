//! The extractor's composer in the box's place: what to do with the text as
//! chips, then the text itself over as many rows as it needs, up to a few,
//! line breaks kept. The field taking the keys carries its label bright and
//! the cursor; the key that does something there sits on the last row's right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::Look;
use super::draft::clip_left;
use crate::support::extract::Operation;
use crate::tui::chat::{ChatPane, ExtractDraft, ExtractField};
use crate::tui::palette::{BOX_IDLE, BOX_LIVE, CHIP_ON, LINE};
use crate::tui::text;
use crate::tui::ui::card::Card;
use crate::tui::ui::{ACCENT, BOLD, CURSOR, DIM, INK, SOFT, spinner};

/// The label column: `operation` and two cells of air.
const LABEL_WIDTH: usize = 11;
/// The most rows the text takes; a longer one shows the rows around the
/// cursor.
const TEXT_ROWS: usize = 4;

/// How tall the composer is at `width` cells: the operation, the text's rows,
/// and the border.
pub(super) fn height(draft: &ExtractDraft, width: u16) -> u16 {
    let room = text_room(width, draft);
    let (rows, _) = draft.text.wrapped(room);
    (1 + rows.len().clamp(1, TEXT_ROWS) + 2) as u16
}

/// The cells the text wraps at in a composer `width` cells wide.
fn text_room(width: u16, draft: &ExtractDraft) -> usize {
    let hint = hint(draft, false).iter().map(Span::width).sum::<usize>();
    (width as usize)
        .saturating_sub(4 + LABEL_WIDTH + hint + 2)
        .max(8)
}

/// Draw the composer into `area`.
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    pane: &ChatPane,
    draft: &ExtractDraft,
    look: &Look,
) {
    let streaming = pane.streaming();
    let border = if streaming {
        LINE
    } else if draft.text.is_empty() {
        BOX_IDLE
    } else {
        BOX_LIVE
    };
    let title = if streaming {
        vec![
            Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT),
            Span::styled("reading", SOFT),
        ]
    } else {
        Vec::new()
    };
    Card::new(title)
        .border(Style::new().fg(border))
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let hint = hint(draft, streaming);
    let hint_width: usize = hint.iter().map(Span::width).sum();
    let blink = look.blink_on() && !streaming;
    let on_text = draft.field() == ExtractField::Text;

    let mut lines = vec![Line::from({
        let focused = draft.field() == ExtractField::Operation;
        // A chip carries a cell of its own before its word, so the label
        // gives that cell back to keep the words in one column.
        let mut spans = vec![Span::styled(
            format!("{:<width$}", "operation", width = LABEL_WIDTH - 1),
            if focused { BOLD } else { DIM },
        )];
        spans.extend(operations(draft, focused));
        spans
    })];
    let room = text_room(area.width, draft);
    let label = |first: bool| {
        Span::styled(
            format!("{:<LABEL_WIDTH$}", if first { "text" } else { "" }),
            if on_text { BOLD } else { DIM },
        )
    };
    if draft.text.is_empty() {
        let caret = match (on_text, blink) {
            (true, true) => CURSOR,
            (true, false) => " ",
            (false, _) => "",
        };
        lines.push(Line::from(vec![
            label(true),
            Span::styled(caret, BOLD),
            Span::styled(
                text::clip("type or paste the text to read", room.saturating_sub(1)),
                DIM,
            ),
        ]));
    } else {
        let (rows, (row, column)) = draft.text.wrapped(room);
        let first = row
            .saturating_sub(TEXT_ROWS - 1)
            .min(rows.len().saturating_sub(TEXT_ROWS));
        for (index, piece) in rows.iter().enumerate().skip(first).take(TEXT_ROWS) {
            let mut spans = vec![label(index == first)];
            // A tab shows as the one cell `wrapped` counted it as.
            let piece = piece.replace('\t', " ");
            if on_text && index == row {
                let (before, after) = split_at_cells(&piece, column);
                spans.push(Span::styled(before, INK));
                spans.push(Span::styled(if blink { CURSOR } else { " " }, BOLD));
                spans.push(Span::styled(after, INK));
            } else {
                spans.push(Span::styled(clip_left(&piece, room), INK));
            }
            lines.push(Line::from(spans));
        }
    }
    if let Some(last) = lines.last_mut() {
        let used = last.width();
        let pad = (inner.width as usize).saturating_sub(used + hint_width);
        last.spans.push(Span::raw(" ".repeat(pad)));
        last.spans.extend(hint);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The operations as chips, the one asked raised; brighter while the field
/// has the keys.
fn operations(draft: &ExtractDraft, focused: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, operation) in Operation::ALL.iter().enumerate() {
        if index == draft.operation {
            spans.push(Span::styled(
                format!(" {} ", operation.as_str()),
                if focused { BOLD } else { INK }.bg(CHIP_ON),
            ));
        } else {
            spans.push(Span::styled(format!(" {} ", operation.as_str()), DIM));
        }
        spans.push(Span::raw(" "));
    }
    spans
}

/// `piece` cut at `column` cells.
fn split_at_cells(piece: &str, column: usize) -> (String, String) {
    let mut used = 0;
    let mut at = piece.len();
    for (offset, c) in piece.char_indices() {
        if used >= column {
            at = offset;
            break;
        }
        used += c.to_string().width();
    }
    (piece[..at].to_owned(), piece[at..].to_owned())
}

/// The key that does something in the field taking the keys.
fn hint(draft: &ExtractDraft, streaming: bool) -> Vec<Span<'static>> {
    let (key, verb) = if streaming {
        ("esc", " stop")
    } else {
        match draft.field() {
            ExtractField::Operation => ("←→", " operation"),
            ExtractField::Text if draft.text.is_empty() => ("tab", " operation"),
            ExtractField::Text => ("enter", " read"),
        }
    };
    vec![Span::styled(key, BOLD), Span::styled(verb, DIM)]
}
