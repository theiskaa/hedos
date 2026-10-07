//! A judge's turns as lines: the ask as the parts of the question in the
//! bubble, and the answer as its distribution, one row per outcome with a
//! bar on the track and its share, the model's answer marked and bright.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::support::judge::{self, Kind};
use crate::tui::chat::Asked;
use crate::tui::palette::{BAR_EMPTY, BAR_FILLED, BRIGHT, FAINT_COLOR, INK_COLOR, mix};
use crate::tui::text;
use crate::tui::ui::{BOLD, CAUTION, DIM, INK, SELECTED_MARK, SOFT, TRACK};
use crate::tui::wrap;

/// The widest a label column gets, and the widest a bar.
const LABEL_MAX: usize = 28;
const BAR_MAX: usize = 36;
/// `100.0%` and the cell before it.
const PERCENT_WIDTH: usize = 7;

/// The ask's parts for its bubble, each with its style: the situation, the
/// question, and the options or levels it may be answered with.
pub(super) fn ask(asked: &Asked) -> Vec<(String, Style)> {
    let mut parts = Vec::new();
    if !asked.situation.is_empty() {
        parts.push((asked.situation.clone(), SOFT));
    }
    parts.push((asked.question.instructions.clone(), INK));
    let labels: Vec<&str> = asked
        .question
        .criteria
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();
    match asked.question.kind {
        Kind::Choice => parts.push((labels.join(" · "), DIM)),
        Kind::Score => parts.push((labels.join(" → "), DIM)),
        Kind::Noul => {}
    }
    parts
}

/// What the bubble's label calls the ask.
pub(super) fn kind(asked: &Asked) -> &'static str {
    asked.question.kind.as_str()
}

/// The answer to `asked` in `reply` at `width` cells: a row per outcome,
/// then what the model says of its own certainty. A reply that is no
/// judgment says so, with what came back.
pub(super) fn answer(reply: &str, asked: &Asked, width: usize) -> Vec<Line<'static>> {
    let read = match judge::answer(reply, &asked.question) {
        Ok(read) => read,
        Err(reason) => {
            return wrap::wrap(&reason, width)
                .into_iter()
                .map(|piece| Line::from(Span::styled(piece, CAUTION)))
                .collect();
        }
    };
    let label_width = read
        .outcomes
        .iter()
        .map(|outcome| outcome.label.width())
        .max()
        .unwrap_or(0)
        .min(LABEL_MAX)
        .min(width / 3);
    let bar = width
        .saturating_sub(2 + label_width + 2 + PERCENT_WIDTH)
        .min(BAR_MAX);
    let mut lines = Vec::new();
    for outcome in &read.outcomes {
        let share = outcome.probability.clamp(0.0, 1.0);
        let filled = ((share * bar as f64).round() as usize).min(bar);
        let filled = filled.max(usize::from(share > 0.0 && bar > 0));
        let (mark, name, fill, figure) = if outcome.chosen {
            (
                Span::styled(format!("{SELECTED_MARK} "), Style::new().fg(BRIGHT)),
                BOLD,
                Style::new().fg(BRIGHT),
                BOLD,
            )
        } else {
            (
                Span::raw("  "),
                SOFT,
                Style::new().fg(mix(FAINT_COLOR, INK_COLOR, 0.45)),
                SOFT,
            )
        };
        let label = text::clip(&outcome.label, label_width);
        let pad = label_width.saturating_sub(label.width());
        lines.push(Line::from(vec![
            mark,
            Span::styled(label, name),
            Span::raw(" ".repeat(pad + 2)),
            Span::styled(BAR_FILLED.repeat(filled), fill),
            Span::styled(BAR_EMPTY.repeat(bar - filled), TRACK),
            Span::styled(format!("{:>PERCENT_WIDTH$}", percent(share)), figure),
        ]));
        if outcome.chosen && !outcome.detail.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(text::clip(&outcome.detail, width.saturating_sub(2)), DIM),
            ]));
        }
    }
    let mut notes = Vec::new();
    if let Some((score, top)) = read.score {
        notes.push(format!("expected {score:.1} of {top}"));
    }
    if let Some(confidence) = read.confidence {
        notes.push(format!("confidence {}", percent(confidence)));
    }
    if !notes.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            text::clip(&format!("  {}", notes.join(" · ")), width),
            DIM,
        )));
    }
    lines
}

/// The chosen outcome's share and label, for the session card's figure.
pub(super) fn headline(reply: &str, asked: &Asked) -> Option<(f64, String)> {
    let read = judge::answer(reply, &asked.question).ok()?;
    let chosen = read.outcomes.into_iter().find(|outcome| outcome.chosen)?;
    Some((chosen.probability, chosen.label))
}

/// `71.2%`, or `100%` and `0%` at the ends.
fn percent(share: f64) -> String {
    let value = share * 100.0;
    if value >= 99.95 {
        "100%".to_owned()
    } else if value < 0.05 {
        "0%".to_owned()
    } else {
        format!("{value:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::support::judge::Question;
    use crate::tui::testing::texts;

    fn asked(kind: Kind, criteria: &[(&str, &str)]) -> Asked {
        Asked {
            situation: "the order arrived broken".to_owned(),
            question: Question {
                id: "q1".to_owned(),
                kind,
                instructions: "how should support answer?".to_owned(),
                criteria: criteria
                    .iter()
                    .map(|(label, detail)| ((*label).to_owned(), (*detail).to_owned()))
                    .collect(),
            },
        }
    }

    #[test]
    fn a_choice_ranks_its_options_and_marks_the_answer() {
        let asked = asked(
            Kind::Choice,
            &[("refund", "money back"), ("replace", ""), ("apologize", "")],
        );
        let reply = r#"{"answers":{"q1":{"choice":"refund","probabilities":{"refund":0.712,"replace":0.2,"apologize":0.088},"confidence":0.8}}}"#;
        let lines = answer(reply, &asked, 60);
        let read = texts(&lines);
        assert!(read[0].starts_with("▌ refund"), "{read:?}");
        assert!(read[0].ends_with(" 71.2%"));
        assert_eq!(read[1].trim(), "money back");
        assert!(read[2].starts_with("  replace"));
        assert_eq!(
            read.last().map(|line| line.trim()),
            Some("confidence 80.0%")
        );
        for line in &lines {
            assert!(line.width() <= 60, "{:?}", read);
        }
    }

    #[test]
    fn a_score_keeps_its_levels_in_order_and_a_noul_reads_yes_and_no() {
        let score = asked(Kind::Score, &[("mild", ""), ("dire", "")]);
        let read = texts(&answer(
            r#"{"answers":{"q1":{"score":0.8,"probabilities":{"0":0.2,"1":0.8}}}}"#,
            &score,
            60,
        ));
        assert!(read[0].starts_with("  0 mild"));
        assert!(read[1].starts_with("▌ 1 dire"));
        assert_eq!(
            read.last().map(|line| line.trim()),
            Some("expected 0.8 of 1")
        );
        let noul = asked(Kind::Noul, &[]);
        let read = texts(&answer(r#"{"answers":{"q1":{"noul":0.73}}}"#, &noul, 60));
        assert!(read[0].starts_with("▌ yes") && read[0].ends_with("73.0%"));
        assert!(read[1].starts_with("  no"));
    }

    #[test]
    fn prose_where_a_judgment_belongs_is_said_plainly() {
        let noul = asked(Kind::Noul, &[]);
        let read = texts(&answer("I think so.", &noul, 60));
        assert!(read[0].contains("did not answer with a judgment"));
    }

    #[test]
    fn the_ask_carries_its_parts_in_order() {
        let parts = ask(&asked(Kind::Choice, &[("a", ""), ("b", "")]));
        let read: Vec<&str> = parts.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(
            read,
            [
                "the order arrived broken",
                "how should support answer?",
                "a · b"
            ]
        );
    }
}
