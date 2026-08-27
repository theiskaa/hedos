//! The task strip: one line per background task, newest last.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use kernel::install::event::InstallProgress;

use super::{ACCENT, BOLD, DIM, FAILED, bar, key_spans};
use crate::tui::app::App;
use crate::tui::strip::TaskRow;
use crate::tui::tasks::{TaskKind, TaskState};
use crate::tui::text;

/// Cells the download bar spans.
const BAR_WIDTH: usize = 24;

/// Draw the strip into `area`, the newest tasks winning when it is short.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" tasks ").border_style(DIM);
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app
        .tasks
        .rows()
        .iter()
        .rev()
        .take(visible)
        .rev()
        .map(line)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn line(row: &TaskRow) -> Line<'static> {
    let activity = row.kind.as_ref().map_or("", TaskKind::activity);
    let (verb_style, detail) = match &row.state {
        TaskState::Running => (ACCENT, vec![Span::styled(activity, DIM)]),
        TaskState::Status(status) => (ACCENT, vec![Span::styled(status.clone(), DIM)]),
        TaskState::Downloading(progress) => (ACCENT, download(progress)),
        TaskState::Done(summary) => {
            let mut spans = vec![Span::styled(summary.clone(), DIM)];
            if matches!(row.kind, Some(TaskKind::Pull(_))) {
                spans.extend(hints(&[("w", "warm"), ("l", "launch")]));
            }
            (DIM, spans)
        }
        TaskState::Failed(reason) => {
            let mut spans = vec![Span::raw(reason.clone())];
            spans.extend(hints(&[("d", "dismiss")]));
            (FAILED, spans)
        }
    };
    let mut spans = vec![
        Span::styled(format!(" {:<6} ", row.label.verb), verb_style),
        Span::raw(format!("{}  ", row.label.subject)),
    ];
    spans.extend(detail);
    Line::from(spans)
}

/// A bar and figures when the total is firm, bytes so far when it is not.
fn download(progress: &InstallProgress) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
    let mut spans = match (progress.fraction(), progress.total_bytes) {
        (Some(fraction), Some(total)) => {
            let filled = (fraction * BAR_WIDTH as f64).round() as usize;
            let mut spans = bar(filled, BAR_WIDTH, ACCENT).to_vec();
            spans.push(Span::styled(
                format!("  {:>3}%", (fraction * 100.0) as u64),
                BOLD,
            ));
            spans.push(Span::styled(
                format!("  {done} / {}", text::bytes(total)),
                DIM,
            ));
            spans
        }
        _ => vec![Span::styled(format!("{done} so far"), DIM)],
    };
    spans.extend(hints(&[("c", "cancel")]));
    spans
}

/// The keys a row offers, set off from its detail by a gap.
fn hints(pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("  ")];
    spans.extend(key_spans(pairs));
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::style::Style;

    use crate::tui::strip::TaskStrip;
    use crate::tui::tasks::TaskLabel;
    use crate::tui::tasks::{TaskEvent, TaskId};
    use crate::tui::testing::{line_text as text, plan};

    fn recorded(state: TaskState) -> TaskRow {
        let mut strip = TaskStrip::default();
        strip.record(
            TaskLabel {
                verb: "remove",
                subject: "mistral".to_owned(),
            },
            state,
            0,
        );
        strip.rows()[0].clone()
    }

    #[test]
    fn a_failed_row_keeps_the_reason_plain_and_offers_dismiss() {
        let row = recorded(TaskState::Failed("is warm; unload it first".to_owned()));
        let line = line(&row);
        assert_eq!(
            text(&line).trim_end(),
            " remove mistral  is warm; unload it first  d dismiss"
        );
        assert_eq!(line.spans[0].style, FAILED);
        let reason = line
            .spans
            .iter()
            .find(|span| span.content.contains("unload"))
            .map(|span| span.style);
        assert_eq!(reason, Some(Style::new()));
    }

    #[test]
    fn a_done_pull_hints_in_the_key_verb_grammar() {
        let mut strip = TaskStrip::default();
        let id = TaskId::next();
        strip.start(id, TaskKind::Pull(plan("gemma3")));
        let row = strip
            .moved(
                TaskEvent {
                    id,
                    state: TaskState::Done("pulled gemma3".to_owned()),
                },
                0,
            )
            .cloned();
        let row = row.unwrap();
        assert!(
            text(&line(&row))
                .trim_end()
                .ends_with("pulled gemma3  w warm  l launch")
        );
    }
}
