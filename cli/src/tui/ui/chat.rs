//! The chat pane: the conversation with one model in the body, the prompt
//! line under it. The user's turns speak in their own hue, replies are plain,
//! a spinner turns until the first token, and how a reply ended sits under it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{ACCENT, BOLD, CAUTION, CURSOR, DIM, FAILED, VOICE};
use crate::tui::app::App;
use crate::tui::chat::{ChatPane, Ending, Speaker, Turn, View};
use crate::tui::edit::LineEdit;
use crate::tui::text;

/// The prompt marker, its width in cells, and the indent replies share.
const MARK: &str = "› ";
const MARK_CELLS: usize = 2;
const INDENT: &str = "  ";
/// Rows under the transcript: the rule and the prompt line.
const PROMPT_ROWS: u16 = 2;
/// The spinner shown until the first token, one glyph per tick.
const SPINNER: [&str; 6] = ["⠋", "⠙", "⠸", "⠴", "⠦", "⠇"];

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
    let visible = transcript_area.height as usize;
    pane.measured(lines.len().saturating_sub(visible));
    let first = pane.first_line();
    let mut spans = vec![Span::styled(title, ACCENT)];
    if pane.view != View::Follow {
        spans.push(Span::styled(
            format!("line {} of {} ", first + 1, lines.len()),
            DIM,
        ));
    }
    frame.render_widget(block.title(Line::from(spans)), area);
    let shown: Vec<Line> = lines.into_iter().skip(first).take(visible).collect();
    frame.render_widget(Paragraph::new(shown), transcript_area);
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(inner.width as usize), DIM)),
        rule,
    );
    frame.render_widget(
        Paragraph::new(prompt_line(
            &pane.input,
            width.saturating_sub(MARK_CELLS + 1),
        )),
        prompt,
    );
}

/// Every turn wrapped to `width`, a blank line between turns; a hint when
/// nothing has been said yet.
fn transcript(pane: &ChatPane, width: usize, ticks: u64) -> Vec<Line<'static>> {
    if pane.turns.is_empty() {
        return vec![
            Line::default(),
            Line::from(Span::styled(
                format!(
                    "{INDENT}ask {} anything; the conversation stays until the pane closes",
                    pane.record.display_name()
                ),
                DIM,
            )),
        ];
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
            for (index, line) in text::wrap(&turn.text, body).into_iter().enumerate() {
                let lead = if index == 0 { MARK } else { INDENT };
                lines.push(Line::from(Span::styled(format!("{lead}{line}"), VOICE)));
            }
        }
        Speaker::Model => {
            if turn.text.is_empty() && turn.ending == Ending::Open {
                let glyph = SPINNER[(ticks % SPINNER.len() as u64) as usize];
                lines.push(Line::from(vec![
                    Span::styled(format!("{INDENT}{glyph}"), ACCENT),
                    Span::styled(" thinking", DIM),
                ]));
            } else if !turn.text.is_empty() {
                for line in text::wrap(&turn.text, body) {
                    lines.push(Line::from(format!("{INDENT}{line}")));
                }
            }
            let ending = match &turn.ending {
                Ending::Open | Ending::Done(None) => None,
                Ending::Done(Some(stats)) => Some(Span::styled(format!("{INDENT}{stats}"), DIM)),
                Ending::Stopped => Some(Span::styled(format!("{INDENT}stopped"), CAUTION)),
                Ending::Failed(reason) => {
                    Some(Span::styled(format!("{INDENT}failed: {reason}"), FAILED))
                }
            };
            if let Some(span) = ending {
                lines.push(Line::from(span));
            }
        }
    }
    lines
}

/// `› text▏`: what is being typed, with the cursor kept on screen.
fn prompt_line(input: &LineEdit, width: usize) -> Line<'static> {
    let (before, after) = input.view(width);
    Line::from(vec![
        Span::styled(MARK, VOICE),
        Span::raw(before),
        Span::styled(CURSOR, BOLD),
        Span::raw(after),
    ])
}
