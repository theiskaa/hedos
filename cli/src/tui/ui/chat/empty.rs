//! What the transcript shows before anything is asked: the model's name and
//! facts, and three things to ask, the one `tab` puts in the box next marked.
//! A judge's shows what it can be asked instead, the kind chosen marked.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Look;
use crate::support::judge::Kind;
use crate::support::shelf_table::runtime_label;
use crate::tui::chat::{ChatPane, KINDS, SUGGESTIONS};
use crate::tui::text;
use crate::tui::ui::{BOLD, DIM, EYEBROW, INK, SOFT, centered};

/// How fast the name arrives.
const REVEAL_PER_MS: u64 = 4;
/// The width the block is laid out to, centred in the transcript.
const BLOCK_WIDTH: u16 = 52;

/// Draw the empty state into `area`, the transcript's inside.
pub(super) fn draw(frame: &mut Frame, area: Rect, pane: &ChatPane, look: &Look) {
    let lines = lines(pane, look, BLOCK_WIDTH.min(area.width) as usize);
    let rect = centered(area, BLOCK_WIDTH.min(area.width), lines.len() as u16);
    frame.render_widget(Paragraph::new(lines), rect);
}

fn lines(pane: &ChatPane, look: &Look, width: usize) -> Vec<Line<'static>> {
    let record = &pane.record;
    let name = crate::support::text::printable(record.display_name()).into_owned();
    let mut name_line = Vec::new();
    for (piece, ramp) in
        look.motion
            .reveal(&text::clip(&name, width), pane.opened_at, REVEAL_PER_MS)
    {
        name_line.push(Span::styled(piece, if ramp { DIM } else { BOLD }));
    }
    let facts = [
        Some(text::short_runtime(runtime_label(record)).to_owned())
            .filter(|runtime| runtime != "—"),
        record.serving_size().map(text::bytes),
        record
            .context_length
            .map(|context| format!("{} context", text::tokens(context))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut lines = vec![
        Line::from(name_line),
        Line::from(Span::styled(text::clip(&facts, width), SOFT)),
        Line::default(),
    ];
    if let Some(draft) = pane.draft.as_deref() {
        lines.push(Line::from(Span::styled("DECIDES, DOESN'T CHAT", EYEBROW)));
        lines.push(Line::default());
        for (index, kind) in KINDS.iter().enumerate() {
            let chosen = index == draft.kind;
            lines.push(Line::from(vec![
                Span::styled(if chosen { "› " } else { "  " }, INK),
                Span::styled(
                    format!("{:<8}", kind.as_str()),
                    if chosen { BOLD } else { SOFT },
                ),
                Span::styled(
                    text::clip(meaning(*kind), width.saturating_sub(10)),
                    if chosen { INK } else { DIM },
                ),
            ]));
            lines.push(Line::default());
        }
        lines.push(Line::from(vec![
            Span::styled("tab", BOLD),
            Span::styled(" moves between the fields below", DIM),
        ]));
        return lines;
    }
    lines.extend([
        Line::from(Span::styled("TRY ASKING", EYEBROW)),
        Line::default(),
    ]);
    for (index, suggestion) in SUGGESTIONS.iter().enumerate() {
        let chosen = index == pane.suggestion();
        lines.push(Line::from(vec![
            Span::styled(if chosen { "› " } else { "  " }, INK),
            Span::styled(
                text::clip(suggestion, width.saturating_sub(2)),
                if chosen { INK } else { SOFT },
            ),
        ]));
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::styled("tab", BOLD),
        Span::styled(" puts one in the box", DIM),
    ]));
    lines
}

/// What a kind of question gets back.
fn meaning(kind: Kind) -> &'static str {
    match kind {
        Kind::Choice => "picks one of your options",
        Kind::Score => "rates against your levels",
        Kind::Noul => "weighs how far a statement holds",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::motion::Motion;
    use crate::tui::testing::{record, texts};

    #[test]
    fn the_empty_state_names_the_model_and_offers_three_things_to_ask() {
        let motion = Motion::settled();
        let look = Look {
            motion: &motion,
            spin_frame: 0,
        };
        let pane = ChatPane::open(record("qwen"));
        let read = texts(&lines(&pane, &look, 52));
        assert_eq!(read[0], "qwen");
        assert_eq!(read[3], "TRY ASKING");
        assert_eq!(read[5], format!("› {}", SUGGESTIONS[0]));
        assert_eq!(read[7], format!("  {}", SUGGESTIONS[1]));
        assert_eq!(
            read.last().map(String::as_str),
            Some("tab puts one in the box")
        );
    }

    #[test]
    fn a_judges_empty_state_offers_the_kinds_of_question() {
        let motion = Motion::settled();
        let look = Look {
            motion: &motion,
            spin_frame: 0,
        };
        let mut judge = record("laya");
        judge.capabilities = vec![kernel::records::Capability::judge()];
        let pane = ChatPane::open(judge);
        let read = texts(&lines(&pane, &look, 52));
        assert_eq!(read[3], "DECIDES, DOESN'T CHAT");
        assert_eq!(read[5], "› choice  picks one of your options");
        assert_eq!(read[9], "  noul    weighs how far a statement holds");
    }
}
