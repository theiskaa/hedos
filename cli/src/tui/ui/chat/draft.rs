//! The judge's composer in the box's place: a row for each part of a typed
//! question, the kind as chips, the situation, the question, and the options
//! or levels given so far with the next one being typed. The field taking
//! the keys carries its label bright and the cursor; the key that does
//! something there sits on the last row's right.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::Look;
use crate::support::judge::Kind;
use crate::tui::chat::{ChatPane, Draft, Field, KINDS};
use crate::tui::edit::LineEdit;
use crate::tui::palette::{BOX_IDLE, BOX_LIVE, CHIP_ON, LINE};
use crate::tui::text;
use crate::tui::ui::card::Card;
use crate::tui::ui::{ACCENT, BOLD, CURSOR, DIM, INK, SOFT, spinner};

/// The label column: `situation` and two cells of air.
const LABEL_WIDTH: usize = 11;

/// How tall the composer is: a row for each field and its border.
pub(super) fn height(draft: &Draft) -> u16 {
    draft.fields().len() as u16 + 2
}

/// Draw the composer into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, pane: &ChatPane, draft: &Draft, look: &Look) {
    let streaming = pane.streaming();
    let typed = !(draft.situation.is_empty()
        && draft.question.is_empty()
        && draft.option.is_empty()
        && draft.options.is_empty());
    let border = if streaming {
        LINE
    } else if typed {
        BOX_LIVE
    } else {
        BOX_IDLE
    };
    let title = if streaming {
        vec![
            Span::styled(format!("{} ", spinner(look.spin_frame)), ACCENT),
            Span::styled("weighing", SOFT),
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
    let room = (inner.width as usize).saturating_sub(LABEL_WIDTH + hint_width + 2);
    let blink = look.blink_on() && !streaming;
    let fields = draft.fields();
    let mut lines: Vec<Line> = fields
        .iter()
        .map(|field| {
            let focused = *field == draft.field();
            // A chip carries a cell of its own before its word, so the kind's
            // label gives that cell back to keep the words in one column.
            let label_width = LABEL_WIDTH - usize::from(*field == Field::Kind);
            let mut spans = vec![Span::styled(
                format!("{:<label_width$}", label(*field, draft.kind())),
                if focused { BOLD } else { DIM },
            )];
            // The field with the keys keeps its cursor's cell while it
            // blinks off, so its text never shifts.
            let cursor = focused.then_some(blink);
            spans.extend(match field {
                Field::Kind => kinds(draft, focused),
                Field::Situation => typed_or(
                    &draft.situation,
                    room,
                    cursor,
                    "what is being judged; optional",
                ),
                Field::Question => typed_or(&draft.question, room, cursor, prompt(draft.kind())),
                Field::Options => options(draft, room, cursor),
            });
            Line::from(spans)
        })
        .collect();
    if let Some(last) = lines.last_mut() {
        let used = last.width();
        let pad = (inner.width as usize).saturating_sub(used + hint_width);
        last.spans.push(Span::raw(" ".repeat(pad)));
        last.spans.extend(hint);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// A field's label.
fn label(field: Field, kind: Kind) -> &'static str {
    match (field, kind) {
        (Field::Kind, _) => "kind",
        (Field::Situation, _) => "situation",
        (Field::Question, Kind::Noul) => "statement",
        (Field::Question, _) => "question",
        (Field::Options, Kind::Score) => "levels",
        (Field::Options, _) => "options",
    }
}

/// What the question field says while it is empty.
fn prompt(kind: Kind) -> &'static str {
    match kind {
        Kind::Choice => "what should be decided about it",
        Kind::Score => "what is being rated",
        Kind::Noul => "something that may or may not hold",
    }
}

/// The kinds as chips, the one asked raised; brighter while the field has
/// the keys.
fn kinds(draft: &Draft, focused: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, kind) in KINDS.iter().enumerate() {
        if index == draft.kind {
            spans.push(Span::styled(
                format!(" {} ", kind.as_str()),
                if focused { BOLD } else { INK }.bg(CHIP_ON),
            ));
        } else {
            spans.push(Span::styled(format!(" {} ", kind.as_str()), DIM));
        }
        spans.push(Span::raw(" "));
    }
    spans
}

/// What was typed into `input`, cut to `room` around its cursor, or the
/// placeholder; `cursor` is whether the blinking cursor shows, `None` on a
/// field without the keys.
pub(super) fn typed_or(
    input: &LineEdit,
    room: usize,
    cursor: Option<bool>,
    placeholder: &str,
) -> Vec<Span<'static>> {
    let caret = Span::styled(
        match cursor {
            Some(true) => CURSOR,
            Some(false) => " ",
            None => "",
        },
        BOLD,
    );
    if cursor.is_none() && !input.is_empty() {
        // A field without the keys reads from its start.
        return vec![Span::styled(text::clip(input.as_str(), room), INK)];
    }
    if input.is_empty() {
        return vec![
            caret,
            Span::styled(text::clip(placeholder, room.saturating_sub(1)), DIM),
        ];
    }
    let (before, after) = input.view(room.saturating_sub(1));
    vec![Span::styled(before, INK), caret, Span::styled(after, INK)]
}

/// The options given so far, joined, then the one being typed. The oldest
/// give way first when the row runs out.
fn options(draft: &Draft, room: usize, cursor: Option<bool>) -> Vec<Span<'static>> {
    let mut given = draft
        .entries()
        .into_iter()
        .map(|(label, _)| label)
        .collect::<Vec<_>>()
        .join(" · ");
    if !given.is_empty() && (cursor.is_some() || !draft.option.is_empty()) {
        given.push_str(" · ");
    }
    let typing = if draft.option.is_empty() {
        0
    } else {
        draft.option.as_str().width() + 1
    };
    let keep = room.saturating_sub(typing.min(room / 2));
    let given = clip_left(&given, keep);
    let room = room.saturating_sub(given.width());
    let placeholder = if draft.options.is_empty() {
        match draft.kind() {
            Kind::Score => "lowest first; enter adds each",
            _ => "a label, or label: what it means",
        }
    } else {
        ""
    };
    let mut spans = vec![Span::styled(given, SOFT)];
    spans.extend(typed_or(&draft.option, room, cursor, placeholder));
    spans
}

/// The end of `text` that fits `width` cells, an ellipsis where it was cut.
pub(super) fn clip_left(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut kept = String::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let cells = c.to_string().width();
        if used + cells > width {
            break;
        }
        used += cells;
        kept.insert(0, c);
    }
    format!("…{kept}")
}

/// The key that does something in the field taking the keys.
fn hint(draft: &Draft, streaming: bool) -> Vec<Span<'static>> {
    let (key, verb) = if streaming {
        ("esc", " stop")
    } else {
        match draft.field() {
            Field::Kind => ("←→", " kind"),
            Field::Question if draft.kind() == Kind::Noul => ("enter", " judge"),
            Field::Situation | Field::Question => ("tab", " next"),
            Field::Options if !draft.option.is_empty() || draft.options.len() < 2 => {
                ("enter", " add")
            }
            Field::Options => ("enter", " judge"),
        }
    };
    vec![Span::styled(key, BOLD), Span::styled(verb, DIM)]
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::records::Capability;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::tui::event::Key;
    use crate::tui::motion::Motion;
    use crate::tui::testing::record;

    fn judge() -> ChatPane {
        let mut record = record("laya");
        record.capabilities = vec![Capability::judge()];
        ChatPane::open(record)
    }

    fn drawn(pane: &ChatPane, width: u16) -> Vec<String> {
        let draft = pane.draft.as_deref().expect("a judge");
        let height = height(draft);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
        let motion = Motion::settled();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    pane,
                    draft,
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
    fn a_fresh_composer_offers_every_part_of_a_question() {
        let rows = drawn(&judge(), 80);
        assert_eq!(rows.len(), 6);
        assert!(rows[1].contains("kind") && rows[1].contains(" choice "));
        assert!(
            rows[2].contains("situation  ▏what is being judged"),
            "{:?}",
            rows[2]
        );
        assert!(rows[3].contains("question"));
        assert!(rows[4].contains("options") && rows[4].contains("tab next"));
    }

    #[test]
    fn given_options_line_up_before_the_one_being_typed() {
        let mut pane = judge();
        pane.next_field(2);
        for word in ["refund", "replace"] {
            for c in word.chars() {
                pane.edit(Key::Char(c));
            }
            pane.judge_enter(0, 0, true);
        }
        pane.edit(Key::Char('a'));
        let rows = drawn(&pane, 80);
        assert!(rows[4].contains("refund · replace · a▏"), "{:?}", rows[4]);
        assert!(rows[4].contains("enter add"));
    }

    #[test]
    fn a_noul_drops_the_options_row() {
        let mut pane = judge();
        pane.next_field(-1);
        pane.edit(Key::Char(' '));
        pane.edit(Key::Char(' '));
        let rows = drawn(&pane, 80);
        assert_eq!(rows.len(), 5);
        assert!(rows[3].contains("statement"));
    }

    #[test]
    fn the_oldest_options_give_way_first() {
        assert_eq!(clip_left("one · two · ", 8), "… two · ");
        assert_eq!(clip_left("ab", 8), "ab");
    }
}
