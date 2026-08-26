//! The task strip: one line per background task, newest last.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{BOLD, DIM, FAILED};
use crate::tui::app::{App, TaskRow};
use crate::tui::tasks::TaskState;

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
        TaskState::Running => (BOLD, Span::styled(row.kind.activity(), DIM)),
        TaskState::Done(summary) => (DIM, Span::styled(summary.clone(), DIM)),
        TaskState::Failed(reason) => (FAILED, Span::styled(reason.clone(), FAILED)),
    };
    Line::from(vec![
        Span::styled(format!(" {:<6} ", row.kind.verb()), verb_style),
        Span::raw(format!("{}  ", row.kind.subject())),
        detail,
    ])
}
