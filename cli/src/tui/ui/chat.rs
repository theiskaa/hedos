//! The chat pane: the conversation with one model in the body, the prompt
//! line under it. The user's turns are bright, replies are plain with their
//! markdown emphasis honoured, a spinner turns until the first token, and how a
//! reply ended sits under it, wrapped like the reply. No hue in the body: the
//! accent is brightness.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{ACCENT, BOLD, CAUTION, DIM, FAILED, edited, spinner};
use crate::tui::app::App;
use crate::tui::chat::{ChatPane, Ending, Speaker, Turn, View};
use crate::tui::markup::{self, Emphasis};
use crate::tui::text;
use crate::tui::wrap;

/// The prompt marker and the indent replies share under it.
const MARK: &str = "› ";
const INDENT: &str = "  ";
/// Rows under the transcript: the rule and the prompt line.
const PROMPT_ROWS: u16 = 2;

/// Draw the chat pane into `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let ticks = app.ticks();
    let Some(pane) = app.chat_pane_mut() else {
        return;
    };
    let turns = pane
        .turns
        .iter()
        .filter(|turn| turn.speaker == Speaker::User)
        .count();
    let title = if turns == 0 {
        format!(" try {} ", pane.record.display_name())
    } else {
        format!(
            " try {} · {} ",
            pane.record.display_name(),
            text::count(turns, "turn")
        )
    };
    let block = Block::bordered().border_style(DIM);
    let inner = block.inner(area);
    if inner.height < PROMPT_ROWS + 1 {
        frame.render_widget(block.title(Span::styled(title, ACCENT)), area);
        return;
    }
    let [transcript_area, rule, prompt] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // A cell is kept free on the right so a full line never touches the border.
    let width = inner.width.saturating_sub(1) as usize;
    let lines = transcript(pane, width, ticks);
    let room = transcript_area.height as usize;
    pane.measured(lines.len().saturating_sub(room));
    let first = pane.first_line();
    let mut spans = vec![Span::styled(title, ACCENT)];
    if pane.view != View::Follow {
        spans.push(Span::styled(
            format!("line {} of {} ", first + 1, lines.len()),
            DIM,
        ));
    }
    frame.render_widget(block.title(Line::from(spans)), area);
    let shown: Vec<Line> = lines.into_iter().skip(first).take(room).collect();
    frame.render_widget(Paragraph::new(shown), transcript_area);
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(inner.width as usize), DIM)),
        rule,
    );
    let placeholder = format!("ask {} anything", pane.record.display_name());
    frame.render_widget(
        Paragraph::new(Line::from(edited(&pane.input, MARK, width, &placeholder))),
        prompt,
    );
}

/// Every turn wrapped to `width`, a blank line between turns; nothing when
/// nothing has been said yet, since the prompt's placeholder says what to do.
fn transcript(pane: &ChatPane, width: usize, ticks: u64) -> Vec<Line<'static>> {
    if pane.turns.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![Line::default()];
    for turn in &pane.turns {
        lines.extend(turn_lines(turn, width, ticks));
        lines.push(Line::default());
    }
    lines
}

/// One turn: the user's words in their hue, a reply plain with its ending
/// under it, and the spinner while nothing has come back yet.
fn turn_lines(turn: &Turn, width: usize, ticks: u64) -> Vec<Line<'static>> {
    let body = width.saturating_sub(INDENT.len());
    let mut lines = Vec::new();
    match turn.speaker {
        Speaker::User => {
            for (index, line) in wrap::wrap(&turn.text, body).into_iter().enumerate() {
                let lead = if index == 0 { MARK } else { INDENT };
                lines.push(Line::from(vec![
                    Span::styled(lead, DIM),
                    Span::styled(line, BOLD),
                ]));
            }
        }
        Speaker::Model => {
            if turn.text.is_empty() && turn.ending == Ending::Open {
                lines.push(Line::from(vec![
                    Span::styled(format!("{INDENT}{}", spinner(ticks)), ACCENT),
                    Span::styled(" thinking", DIM),
                ]));
            } else if !turn.text.is_empty() {
                for line in markup::lines(&turn.text, body) {
                    let mut spans = vec![Span::raw(INDENT)];
                    spans.extend(line.into_iter().map(|run| match run.emphasis {
                        Emphasis::Plain | Emphasis::Code => Span::raw(run.text),
                        Emphasis::Bold => Span::styled(run.text, BOLD),
                    }));
                    lines.push(Line::from(spans));
                }
            }
            let ending = match &turn.ending {
                Ending::Open | Ending::Done(None) => None,
                Ending::Done(Some(stats)) => text::stats(stats).map(|stats| (stats, DIM)),
                Ending::Stopped => Some(("stopped".to_owned(), CAUTION)),
                Ending::Failed(reason) => Some((format!("failed: {reason}"), FAILED)),
            };
            if let Some((ending, style)) = ending {
                for piece in wrap::wrap(&ending, body) {
                    lines.push(Line::from(Span::styled(format!("{INDENT}{piece}"), style)));
                }
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::testing::{record, text, texts};

    fn pane_with(text: &str, ending: Ending) -> ChatPane {
        let mut pane = ChatPane::open(record("m"));
        pane.turns.push(Turn {
            speaker: Speaker::User,
            text: "hi there".to_owned(),
            ending: Ending::Done(None),
        });
        pane.turns.push(Turn {
            speaker: Speaker::Model,
            text: text.to_owned(),
            ending,
        });
        pane
    }

    #[test]
    fn an_empty_pane_has_no_transcript() {
        assert!(transcript(&ChatPane::open(record("m")), 60, 0).is_empty());
    }

    #[test]
    fn a_user_turn_leads_with_the_mark_then_indents() {
        let pane = pane_with("", Ending::Open);
        let lines = texts(&transcript(&pane, 8, 0));
        assert_eq!(lines[1], "› hi");
        assert_eq!(lines[2], "  there");
    }

    #[test]
    fn the_spinner_turns_only_before_the_first_token() {
        let waiting = texts(&turn_lines(&pane_with("", Ending::Open).turns[1], 40, 1));
        assert_eq!(waiting, [format!("  {} thinking", spinner(1))]);
        let streaming = texts(&turn_lines(
            &pane_with("word", Ending::Open).turns[1],
            40,
            1,
        ));
        assert_eq!(streaming, ["  word"]);
    }

    #[test]
    fn how_a_reply_ended_sits_under_it() {
        let done = texts(&turn_lines(
            &pane_with("ok", Ending::Done(None)).turns[1],
            40,
            0,
        ));
        assert_eq!(done, ["  ok"]);
        let stopped = texts(&turn_lines(
            &pane_with("ok", Ending::Stopped).turns[1],
            40,
            0,
        ));
        assert_eq!(stopped, ["  ok", "  stopped"]);
        let failed = texts(&turn_lines(
            &pane_with("", Ending::Failed("gone".to_owned())).turns[1],
            40,
            0,
        ));
        assert_eq!(failed, ["  failed: gone"]);
    }

    #[test]
    fn a_failed_reply_wraps_its_reason() {
        let reason = "the runtime went away while the request was in flight, retry".to_owned();
        assert_eq!(reason.len(), 60);
        let lines = turn_lines(&pane_with("", Ending::Failed(reason)).turns[1], 20, 0);
        assert!(lines.len() > 3, "{:?}", texts(&lines));
        for line in &lines {
            assert!(line.width() <= 20, "{:?} runs past 20", text(line));
            assert!(text(line).starts_with("  "));
            assert!(line.spans.iter().all(|span| span.style == FAILED));
        }
        assert!(texts(&lines)[0].starts_with("  failed: the"));
        let stopped = turn_lines(&pane_with("ok", Ending::Stopped).turns[1], 20, 0);
        assert_eq!(texts(&stopped), ["  ok", "  stopped"]);
        assert_eq!(stopped[1].spans[0].style, CAUTION);
    }
}
