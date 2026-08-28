//! The task strip: one line per background task, newest last.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use kernel::install::event::InstallProgress;

use super::{ACCENT, BOLD, DIM, FAILED, bar, key_spans, label_width, padded, right_aligned};
use crate::tui::app::App;
use crate::tui::keymap;
use crate::tui::strip::TaskRow;
use crate::tui::tasks::{TaskKind, TaskState};
use crate::tui::text;

/// The narrowest download bar worth drawing; under it the figures stand
/// alone.
const MIN_BAR_WIDTH: u16 = 8;
/// The widest download bar, however much room the row has.
const MAX_BAR_WIDTH: u16 = 24;
/// Cells the percentage is held to: `100%` at the widest.
const PERCENT_WIDTH: usize = 4;

/// Draw the strip into `area`, the newest tasks winning when it is short.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" tasks ").border_style(DIM);
    let inner = block.inner(area);
    let lines: Vec<Line> = app
        .tasks
        .rows()
        .iter()
        .rev()
        .take(inner.height as usize)
        .rev()
        .map(|row| line(row, inner.width as usize))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// A task's row at `width` cells: verb, subject, then its detail.
fn line(row: &TaskRow, width: usize) -> Line<'static> {
    let verb = format!(
        " {} ",
        padded(row.label.verb, label_width(&TaskKind::VERBS, 0))
    );
    let subject = format!("{}  ", row.label.subject);
    let head = verb.width() + subject.width();
    let activity = row.kind.as_ref().map_or("", TaskKind::activity);
    let (verb_style, detail) = match &row.state {
        TaskState::Running => (ACCENT, vec![Span::styled(activity, DIM)]),
        TaskState::Status(status) => (ACCENT, vec![Span::styled(status.clone(), DIM)]),
        TaskState::Downloading(progress) => {
            (ACCENT, download(progress, width.saturating_sub(head)))
        }
        TaskState::Done(summary) => {
            let mut spans = vec![Span::styled(summary.clone(), DIM)];
            if matches!(row.kind, Some(TaskKind::Pull(_))) {
                spans.extend(hints(&["w", "l"]));
            }
            (DIM, spans)
        }
        TaskState::Failed(reason) => {
            let mut spans = vec![Span::raw(reason.clone())];
            spans.extend(hints(&["d"]));
            (FAILED, spans)
        }
    };
    let mut spans = vec![Span::styled(verb, verb_style), Span::raw(subject)];
    spans.extend(detail);
    Line::from(spans)
}

/// A bar and figures when the total is firm, bytes so far when it is not.
/// The bar takes what `room` leaves after the figures and the cancel key,
/// within its bounds; when that is under the floor the figures stand alone,
/// and the cancel key goes too if it would not fit.
fn download(progress: &InstallProgress, room: usize) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
    let cancel = hints(&["c"]);
    let cancel_width: usize = cancel.iter().map(Span::width).sum();
    let mut spans = match (progress.fraction(), progress.total_bytes) {
        (Some(fraction), Some(total)) => {
            let percent = format!("{}%", (fraction * 100.0) as u64);
            let figures = format!("{done} of {}", text::bytes(total));
            let fixed = PERCENT_WIDTH + 2 + figures.width() + 2 + cancel_width;
            let bar_width = room.saturating_sub(fixed).min(MAX_BAR_WIDTH as usize);
            if bar_width >= MIN_BAR_WIDTH as usize {
                let filled = (fraction * bar_width as f64).round() as usize;
                let mut spans = bar(filled, bar_width, ACCENT).to_vec();
                spans.push(Span::styled(
                    format!("  {}", right_aligned(&percent, PERCENT_WIDTH)),
                    BOLD,
                ));
                spans.push(Span::styled(format!("  {figures}"), DIM));
                spans
            } else {
                let spans = vec![
                    Span::styled(percent, BOLD),
                    Span::styled(format!(" · {figures}"), DIM),
                ];
                let used: usize = spans.iter().map(Span::width).sum();
                if used + cancel_width > room {
                    return spans;
                }
                spans
            }
        }
        _ => vec![Span::styled(format!("{done} so far"), DIM)],
    };
    spans.extend(cancel);
    spans
}

/// The keys a row offers, set off from its detail by a gap.
fn hints(keys: &[&str]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("  ")];
    spans.extend(key_spans(&keymap::pairs(keys)));
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
    fn every_task_verb_is_listed() {
        use crate::support::harnesses::HARNESSES;
        use crate::tui::effect::HandOff;
        use crate::tui::testing::record;
        use std::path::PathBuf;

        let model = record("m");
        let kinds = [
            TaskKind::Scan,
            TaskKind::Warm {
                id: "m".to_owned(),
                name: "m".to_owned(),
            },
            TaskKind::WarmViaGateway {
                id: "m".to_owned(),
                name: "m".to_owned(),
                port: 1,
            },
            TaskKind::Unload {
                id: "m".to_owned(),
                name: "m".to_owned(),
            },
            TaskKind::Pull(plan("gemma3")),
            TaskKind::Remove {
                id: "m".to_owned(),
                name: "m".to_owned(),
            },
        ];
        let hand_offs = [
            HandOff::Launch {
                harness: &HARNESSES[0],
                program: PathBuf::from("/bin/true"),
                record: Box::new(model.clone()),
            },
            HandOff::Chat {
                record: Box::new(model),
            },
            HandOff::Serve,
        ];
        for kind in &kinds {
            // Exhaustive so a new variant lands here before it lands on screen.
            match kind {
                TaskKind::Scan
                | TaskKind::Warm { .. }
                | TaskKind::WarmViaGateway { .. }
                | TaskKind::Unload { .. }
                | TaskKind::Pull(_)
                | TaskKind::Remove { .. } => {}
            }
            let verb = kind.verb();
            assert!(TaskKind::VERBS.contains(&verb), "{verb} is not listed");
        }
        for hand_off in &hand_offs {
            match hand_off {
                HandOff::Launch { .. } | HandOff::Chat { .. } | HandOff::Serve => {}
            }
            let verb = hand_off.label(1).verb;
            assert!(TaskKind::VERBS.contains(&verb), "{verb} is not listed");
        }
    }

    #[test]
    fn a_failed_row_keeps_the_reason_plain_and_offers_dismiss() {
        let row = recorded(TaskState::Failed("is warm; unload it first".to_owned()));
        let line = line(&row, 120);
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
    fn a_download_offers_cancel_from_the_keymap() {
        let progress = InstallProgress {
            bytes_downloaded: 1 << 30,
            total_bytes: Some(4 << 30),
            ..InstallProgress::default()
        };
        let line = Line::from(download(&progress, 120));
        assert!(text(&line).ends_with("  c cancel  "));
    }

    #[test]
    fn the_bar_shrinks_with_the_strip() {
        let progress = InstallProgress {
            bytes_downloaded: 2 << 30,
            total_bytes: Some(4 << 30),
            ..InstallProgress::default()
        };
        let mut strip = TaskStrip::default();
        let id = TaskId::next();
        strip.start(id, TaskKind::Pull(plan("gemma3")));
        let row = strip
            .moved(
                TaskEvent {
                    id,
                    state: TaskState::Downloading(progress),
                },
                0,
            )
            .cloned()
            .unwrap();
        let bar_cells = |width| {
            let line = line(&row, width);
            assert!(line.width() <= width, "{:?} runs past {width}", text(&line));
            text(&line)
                .chars()
                .filter(|c| *c == '█' || *c == '░')
                .count()
        };
        assert_eq!(bar_cells(120), MAX_BAR_WIDTH as usize);
        assert!(text(&line(&row, 120)).contains("  50%  2 GB of 4 GB"));
        let medium = bar_cells(60);
        let bounds = MIN_BAR_WIDTH as usize..MAX_BAR_WIDTH as usize;
        assert!(bounds.contains(&medium), "{medium}");
        assert_eq!(bar_cells(50), 0);
        let compact = text(&line(&row, 50));
        assert!(compact.contains("50% · 2 GB of 4 GB  c cancel"));
        assert_eq!(bar_cells(40), 0);
        let shed = text(&line(&row, 40));
        assert!(shed.contains("50% · 2 GB of 4 GB") && !shed.contains("c cancel"));
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
            text(&line(&row, 120))
                .trim_end()
                .ends_with("pulled gemma3  w warm  l launch")
        );
    }
}
