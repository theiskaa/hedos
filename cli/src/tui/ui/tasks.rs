//! The task strip: one line per background task, newest last.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use kernel::install::event::InstallProgress;

use super::{BOLD, DIM, FAILED};
use crate::tui::app::{App, TaskRow};
use crate::tui::tasks::TaskState;
use crate::tui::text;

/// Cells the download bar spans.
const BAR_WIDTH: usize = 24;

/// Draw the strip into `area`, the newest tasks winning when it is short.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" tasks ").border_style(DIM);
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app
        .tasks
        .iter()
        .rev()
        .take(visible)
        .rev()
        .map(line)
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn line(row: &TaskRow) -> Line<'static> {
    let (verb_style, detail) = match &row.state {
        TaskState::Running => (BOLD, vec![Span::styled(row.kind.activity(), DIM)]),
        TaskState::Status(status) => (BOLD, vec![Span::styled(status.clone(), DIM)]),
        TaskState::Downloading(progress) => (BOLD, download(progress)),
        TaskState::Done(summary) => (DIM, vec![Span::styled(summary.clone(), DIM)]),
        TaskState::Failed(reason) => (FAILED, vec![Span::styled(reason.clone(), FAILED)]),
    };
    let mut spans = vec![
        Span::styled(format!(" {:<6} ", row.kind.verb()), verb_style),
        Span::raw(format!("{}  ", row.kind.subject())),
    ];
    spans.extend(detail);
    Line::from(spans)
}

/// A bar and figures when the total is firm, bytes so far when it is not.
fn download(progress: &InstallProgress) -> Vec<Span<'static>> {
    let done = text::bytes(progress.bytes_downloaded);
    match (progress.fraction(), progress.total_bytes) {
        (Some(fraction), Some(total)) => {
            let filled = ((fraction * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
            vec![
                Span::raw("█".repeat(filled)),
                Span::styled("░".repeat(BAR_WIDTH - filled), DIM),
                Span::styled(format!("  {:>3}%", (fraction * 100.0) as u64), BOLD),
                Span::styled(format!("  {done} / {}", text::bytes(total)), DIM),
            ]
        }
        _ => vec![Span::styled(format!("{done} so far"), DIM)],
    }
}
