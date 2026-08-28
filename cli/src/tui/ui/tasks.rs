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
use crate::tui::strip::{RowHints, TaskRow};
use crate::tui::tasks::{TaskKind, TaskState};
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
    let height = inner.height as usize;
    let shown = app.tasks.shown(height);
    let targets = app
        .tasks
        .hint_targets(height, |reference| app.selected_is(reference));
    let lines: Vec<Line> = shown
        .iter()
        .map(|row| line(row, inner.width as usize, targets.for_row(row)))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
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
    let cancel = if hinted.cancellable {
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
            download(progress, width.saturating_sub(head), cancel),
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
/// The bar takes what `room` leaves after the figures and `cancel`, the
/// hint of the one download `c` acts on and nothing on the rest, within its
/// bounds; when that is under the floor the figures stand alone, and the
/// hint goes too if it would not fit.
fn download(
    progress: &InstallProgress,
    room: usize,
    cancel: Vec<Span<'static>>,
) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
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
mod tests;
