//! The task strip: one line per background task, newest last. A key hint
//! sits only on the row it acts on: `d` on the newest failure, `c` on the
//! newest running pull, `w` and `l` on a done pull while its model is the
//! selected one.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use kernel::install::event::InstallProgress;

use super::{ACCENT, BOLD, DIM, FAILED, bar, key_spans, label_width, padded, right_aligned};
use crate::tui::app::App;
use crate::tui::keymap;
use crate::tui::strip::{TaskRow, TaskStrip};
use crate::tui::tasks::{TaskId, TaskKind, TaskState};
use crate::tui::text;

/// The narrowest download bar worth drawing; under it the figures stand
/// alone.
const MIN_BAR_WIDTH: u16 = 8;
/// The widest download bar, however much room the row has.
const MAX_BAR_WIDTH: u16 = 24;
/// Cells the percentage is held to: `100%` at the widest.
const PERCENT_WIDTH: usize = 4;

/// Draw the strip into `area`; when it is short, the running rows stay and
/// the oldest finished ones go.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" tasks ").border_style(DIM);
    let inner = block.inner(area);
    let shown = app.tasks.shown(inner.height as usize);
    let targets = HintTargets::of(&app.tasks, &shown, |reference| app.selected_is(reference));
    let lines: Vec<Line> = shown
        .iter()
        .map(|row| line(row, inner.width as usize, targets.for_row(row)))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The rows whose hints act, among those on screen: the newest failure
/// answers `d`, the newest running pull `c`, and the done pulls whose model
/// is selected `w` and `l`.
struct HintTargets {
    failure: Option<TaskId>,
    pull: Option<TaskId>,
    done_on_selected: Vec<TaskId>,
}

/// Which of a row's hints apply to it.
#[derive(Clone, Copy, Default)]
struct RowHints {
    dismissable: bool,
    cancellable: bool,
    /// Whether `w` and `l` would act on the model this row pulled.
    on_selected: bool,
}

impl HintTargets {
    /// The targets among `shown`, the rows the strip draws; a target off
    /// screen gets no hint, and the key does not act on it either.
    /// `is_selected` says whether a pull reference names the selected record.
    fn of(strip: &TaskStrip, shown: &[&TaskRow], is_selected: impl Fn(&str) -> bool) -> Self {
        let visible = |id: TaskId| shown.iter().any(|row| row.id == id);
        let done_on_selected = shown
            .iter()
            .filter(|row| match (&row.kind, &row.state) {
                (Some(TaskKind::Pull(plan)), TaskState::Done(_)) => is_selected(&plan.reference),
                _ => false,
            })
            .map(|row| row.id)
            .collect();
        Self {
            failure: strip.newest_failure().filter(|id| visible(*id)),
            pull: strip.newest_running_pull().filter(|id| visible(*id)),
            done_on_selected,
        }
    }

    fn for_row(&self, row: &TaskRow) -> RowHints {
        RowHints {
            dismissable: self.failure == Some(row.id),
            cancellable: self.pull == Some(row.id),
            on_selected: self.done_on_selected.contains(&row.id),
        }
    }
}

/// A task's row at `width` cells: verb, subject, then its detail, with the
/// hints that act on this row; `c` sits on a pull at any running stage, `w`
/// and `l` on a done pull while its model is selected.
fn line(row: &TaskRow, width: usize, hinted: RowHints) -> Line<'static> {
    let verb = format!(
        " {} ",
        padded(row.label.verb, label_width(&TaskKind::VERBS, 0))
    );
    let subject = format!("{}  ", row.label.subject);
    let head = verb.width() + subject.width();
    let activity = row.kind.as_ref().map_or("", TaskKind::activity);
    let cancel = if hinted.cancellable && matches!(row.kind, Some(TaskKind::Pull(_))) {
        hints(&["c"])
    } else {
        Vec::new()
    };
    let (verb_style, detail) = match &row.state {
        TaskState::Running => {
            let mut spans = vec![Span::styled(activity, DIM)];
            spans.extend(cancel);
            (ACCENT, spans)
        }
        TaskState::Status(status) => {
            let mut spans = vec![Span::styled(status.clone(), DIM)];
            spans.extend(cancel);
            (ACCENT, spans)
        }
        TaskState::Downloading(progress) => (
            ACCENT,
            download(progress, width.saturating_sub(head), hinted.cancellable),
        ),
        TaskState::Done(summary) => {
            let mut spans = vec![Span::styled(summary.clone(), DIM)];
            if hinted.on_selected {
                spans.extend(hints(&["w", "l"]));
            }
            (DIM, spans)
        }
        TaskState::Failed(reason) => {
            let mut spans = vec![Span::raw(reason.clone())];
            if hinted.dismissable {
                spans.extend(hints(&["d"]));
            }
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
/// and the cancel key goes too if it would not fit. Only the download `c`
/// acts on is `cancellable`.
fn download(progress: &InstallProgress, room: usize, cancellable: bool) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
    let cancel = if cancellable {
        hints(&["c"])
    } else {
        Vec::new()
    };
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
        let hinted = RowHints {
            dismissable: true,
            ..RowHints::default()
        };
        let line = line(&row, 120, hinted);
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
        let line = Line::from(download(&progress, 120, true));
        assert!(text(&line).ends_with("  c cancel  "));
        let quiet = Line::from(download(&progress, 120, false));
        assert!(!text(&quiet).contains("cancel"));
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
        let cancellable = RowHints {
            cancellable: true,
            ..RowHints::default()
        };
        let bar_cells = |width| {
            let line = line(&row, width, cancellable);
            assert!(line.width() <= width, "{:?} runs past {width}", text(&line));
            text(&line)
                .chars()
                .filter(|c| *c == '█' || *c == '░')
                .count()
        };
        assert_eq!(bar_cells(120), MAX_BAR_WIDTH as usize);
        assert!(text(&line(&row, 120, cancellable)).contains("  50%  2 GB of 4 GB"));
        let medium = bar_cells(60);
        let bounds = MIN_BAR_WIDTH as usize..MAX_BAR_WIDTH as usize;
        assert!(bounds.contains(&medium), "{medium}");
        assert_eq!(bar_cells(50), 0);
        let compact = text(&line(&row, 50, cancellable));
        assert!(compact.contains("50% · 2 GB of 4 GB  c cancel"));
        assert_eq!(bar_cells(40), 0);
        let shed = text(&line(&row, 40, cancellable));
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
        let selected = RowHints {
            on_selected: true,
            ..RowHints::default()
        };
        assert!(
            text(&line(&row, 120, selected))
                .trim_end()
                .ends_with("pulled gemma3  w warm  l launch")
        );
    }

    #[test]
    fn a_done_pull_hints_only_while_it_is_selected() {
        let mut strip = TaskStrip::default();
        let id = TaskId::next();
        strip.start(id, TaskKind::Pull(plan("gemma3")));
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Done("pulled gemma3".to_owned()),
            },
            0,
        );
        let shown = strip.shown(10);
        let selected = HintTargets::of(&strip, &shown, |reference| reference == "gemma3");
        let line_for = |targets: HintTargets| {
            text(&line(shown[0], 120, targets.for_row(shown[0])))
                .trim_end()
                .to_owned()
        };
        assert!(line_for(selected).ends_with("pulled gemma3  w warm  l launch"));
        let elsewhere = HintTargets::of(&strip, &shown, |reference| reference == "llava");
        assert!(line_for(elsewhere).ends_with("pulled gemma3"));
    }

    /// A strip with `failed` failed scans, then `done` finished ones, then
    /// `running` pulls still downloading, oldest first.
    fn strip_of(failed: usize, done: usize, running: usize) -> TaskStrip {
        let mut strip = TaskStrip::default();
        for index in 0..failed {
            let id = TaskId::next();
            strip.start(id, TaskKind::Scan);
            strip.moved(
                TaskEvent {
                    id,
                    state: TaskState::Failed(format!("failed {index}")),
                },
                0,
            );
        }
        for index in 0..done {
            let id = TaskId::next();
            strip.start(id, TaskKind::Scan);
            strip.moved(
                TaskEvent {
                    id,
                    state: TaskState::Done(format!("done {index}")),
                },
                0,
            );
        }
        for index in 0..running {
            let id = TaskId::next();
            strip.start(id, TaskKind::Pull(plan(&format!("pull-{index}"))));
            strip.moved(
                TaskEvent {
                    id,
                    state: TaskState::Downloading(InstallProgress::default()),
                },
                0,
            );
        }
        strip
    }

    fn rendered(strip: &TaskStrip, height: usize) -> Vec<String> {
        let shown = strip.shown(height);
        let targets = HintTargets::of(strip, &shown, |_| false);
        shown
            .iter()
            .map(|row| {
                text(&line(row, 120, targets.for_row(row)))
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn cancel_sits_on_the_newest_pull_even_while_it_resolves() {
        let mut strip = strip_of(0, 0, 1);
        let id = TaskId::next();
        strip.start(id, TaskKind::Pull(plan("pull-b")));
        let lines = rendered(&strip, 10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("pull-0") && !lines[0].contains("cancel"));
        assert!(
            lines[1].ends_with("pull-b  starting  c cancel"),
            "{:?}",
            lines[1]
        );
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Status("resolving on hf".to_owned()),
            },
            0,
        );
        let lines = rendered(&strip, 10);
        assert!(
            lines[1].ends_with("resolving on hf  c cancel"),
            "{:?}",
            lines[1]
        );
        let mut strip = TaskStrip::default();
        strip.start(TaskId::next(), TaskKind::Scan);
        assert!(!rendered(&strip, 10)[0].contains("cancel"));
    }

    #[test]
    fn a_failure_off_screen_offers_no_dismiss() {
        let strip = strip_of(1, 4, 0);
        let lines = rendered(&strip, 4);
        assert!(
            lines.iter().all(|line| !line.contains("dismiss")),
            "{lines:?}"
        );
        assert!(rendered(&strip, 5)[0].ends_with("failed 0  d dismiss"));
    }

    #[test]
    fn only_the_newest_failure_offers_dismiss() {
        let strip = strip_of(2, 0, 2);
        let lines = rendered(&strip, 10);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].ends_with("failed 0") && !lines[0].contains("dismiss"));
        assert!(lines[1].ends_with("failed 1  d dismiss"));
        assert!(lines[2].contains("so far") && !lines[2].contains("cancel"));
        assert!(lines[3].ends_with("so far  c cancel"), "{:?}", lines[3]);
    }

    #[test]
    fn a_running_download_survives_the_cap() {
        let mut strip = strip_of(0, 0, 1);
        for index in 0..5 {
            let id = TaskId::next();
            strip.start(id, TaskKind::Scan);
            strip.moved(
                TaskEvent {
                    id,
                    state: TaskState::Done(format!("done {index}")),
                },
                0,
            );
        }
        let lines = rendered(&strip, 4);
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with(" pull") && lines[0].ends_with("c cancel"));
        assert!(lines[1].ends_with("done 2"));
        assert!(lines[3].ends_with("done 4"));
        let all = rendered(&strip, 10);
        assert_eq!(all.len(), 6);
        assert!(all[0].starts_with(" pull"));
        let crowded = strip_of(0, 1, 3);
        let lines = rendered(&crowded, 2);
        assert!(lines.iter().all(|line| line.starts_with(" pull")));
        assert!(lines[1].contains("pull-2"));
    }
}
