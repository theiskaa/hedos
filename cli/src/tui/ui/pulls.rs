//! The pulls screen: the list of every job where the shelf goes, and the
//! selected pull where the model's detail goes. The list is the `hedos pull
//! ls` columns without the id, which the detail carries; the detail is what
//! `hedos pull attach` and `hedos pull logs` say, on one pane.

use kernel::install::pulls::PullState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
use unicode_width::UnicodeWidthStr;

use super::{
    ACCENT, BOLD, BORDER_COLUMNS, BORDER_ROWS, CAUTION, DIM, EYEBROW, FAILED, SELECTED_MARK,
    SELECTED_ROW, WARM, centered, field_line, label_width, pane, styled_field, value_width, widest,
};
use crate::support::clock;
use crate::support::pulls::progress;
use crate::tui::app::App;
use crate::tui::jobs::JobRow;
use crate::tui::keymap;
use crate::tui::pulls::PullsScreen;
use crate::tui::text;

/// The column headers, in order: gutter, reference, state, progress.
const HEADERS: [&str; 4] = ["", "REFERENCE", "STATE", "PROGRESS"];
const REFERENCE: usize = 1;
const STATE: usize = 2;
const PROGRESS: usize = 3;
/// Space between columns.
const COLUMN_SPACING: u16 = 2;
/// The fewest cells the reference keeps before the progress column goes:
/// a reference cut to this still reads, and the progress is what a pull is
/// being watched for.
const REFERENCE_MIN: u16 = 12;
/// The labels of the detail; the column is as wide as the widest, plus a gap.
const LABELS: [&str; 9] = [
    "state", "progress", "rate", "note", "id", "from", "to", "started", "updated",
];

/// What the list says when the store is empty.
fn empty_note() -> String {
    format!("no pulls yet · p on the shelf {}s one", keymap::verb("p"))
}

/// The width of the detail's label column.
fn label_column() -> usize {
    label_width(&LABELS, 1)
}

/// Draw the list into `area`, scrolled so the selection stays in view.
pub(super) fn draw_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .title(Span::styled(" pulls ", ACCENT))
        .border_style(DIM);
    if app.pulls.rows().is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let note = Line::from(Span::styled(empty_note(), DIM));
        let rect = centered(inner, note.width() as u16, 1);
        frame.render_widget(Paragraph::new(note), rect);
        return;
    }
    let cells: Vec<[String; 4]> = app.pulls.rows().iter().map(cells).collect();
    let widths = column_widths(&cells);
    let columns = fitting_columns(&widths, area.width);
    let selected = app.pulls.selected();
    let body = app
        .pulls
        .rows()
        .iter()
        .zip(&cells)
        .enumerate()
        .map(|(index, (row, cells))| body_row(row, cells, index == selected, columns));
    let header = Row::new(columns.iter().map(|&column| Cell::from(HEADERS[column]))).style(EYEBROW);
    let constraints = columns.iter().map(|&column| match column {
        REFERENCE => Constraint::Min(widths[REFERENCE].min(REFERENCE_MIN)),
        column => Constraint::Length(widths[column]),
    });
    let table = Table::new(body, constraints)
        .header(header)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(SELECTED_ROW)
        .block(block);
    frame.render_stateful_widget(table, area, &mut app.pulls.table);
}

/// A row's cells: the gutter, the reference, the state, and the progress as
/// `hedos pull ls` shows it.
fn cells(row: &JobRow) -> [String; 4] {
    [
        String::new(),
        row.reference.clone(),
        row.pull_state.to_string(),
        progress(&row.status),
    ]
}

/// The widest of each column's cells, or its header.
fn column_widths(rows: &[[String; 4]]) -> [u16; 4] {
    let mut widths = [0u16; 4];
    for (column, width) in widths.iter_mut().enumerate() {
        let cells: Vec<&str> = rows.iter().map(|row| row[column].as_str()).collect();
        *width = widest(&cells).max(HEADERS[column].width()) as u16;
    }
    widths[0] = 1;
    widths
}

/// The columns that fit in `width`: all four while the reference keeps at
/// least [`REFERENCE_MIN`] cells beside them, else the progress goes so the
/// reference and the state stay readable.
fn fitting_columns(widths: &[u16; 4], width: u16) -> &'static [usize] {
    const ALL: &[usize] = &[0, REFERENCE, STATE, PROGRESS];
    let inner = width.saturating_sub(BORDER_COLUMNS);
    let needed = widths[0]
        + widths[REFERENCE].min(REFERENCE_MIN)
        + widths[STATE]
        + widths[PROGRESS]
        + 3 * COLUMN_SPACING;
    if needed <= inner {
        ALL
    } else {
        &ALL[..PROGRESS]
    }
}

/// One list row: the state in its hue, the whole row dim once the pull has
/// ended, the gutter marked when `selected`.
fn body_row(row: &JobRow, cells: &[String; 4], selected: bool, columns: &[usize]) -> Row<'static> {
    let ended = row.pull_state.is_terminal();
    let style = if ended { DIM } else { Style::new() };
    let cells = columns.iter().map(|&column| match column {
        0 if selected => Cell::from(SELECTED_MARK),
        0 => Cell::from(" "),
        STATE => Cell::from(Span::styled(
            cells[STATE].clone(),
            state_style(row.pull_state),
        )),
        column => Cell::from(cells[column].clone()),
    });
    Row::new(cells).style(style)
}

/// The hue a state wears: in motion, stopped with bytes worth keeping, landed,
/// failed, or over.
fn state_style(state: PullState) -> Style {
    match state {
        PullState::Queued | PullState::Running => ACCENT,
        PullState::Paused | PullState::Interrupted => CAUTION,
        PullState::Done => WARM,
        PullState::Failed => FAILED,
        PullState::Cancelled => DIM,
    }
}

/// Draw the selected pull's detail into `area`: its record, then as much of
/// its history as fits, newest last.
pub(super) fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let Some(row) = app.pulls.selected_row() else {
        frame.render_widget(pane(" pull "), area);
        return;
    };
    let block = Block::bordered()
        .title(Span::styled(
            format!(" {} ", row.descriptor.display_name),
            BOLD,
        ))
        .border_style(DIM);
    let width = area.width.saturating_sub(BORDER_COLUMNS) as usize;
    let height = area.height.saturating_sub(BORDER_ROWS) as usize;
    let lines = lines(row, &app.pulls, width, height);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The detail's lines at `width` cells, at most `height` of them: where the
/// pull is and how fast first, since a stacked pane has room for little
/// else, then the record, then the history's newest lines under a heading.
fn lines(row: &JobRow, screen: &PullsScreen, width: usize, height: usize) -> Vec<Line<'static>> {
    let value_width = value_width(width, label_column());
    let field =
        |label, value: String| field_line(label, text::clip(&value, value_width), label_column());
    let state = match row.status.attempt {
        0 | 1 => row.pull_state.to_string(),
        attempt => format!("{} · attempt {attempt}", row.pull_state),
    };
    let mut lines = vec![
        Line::from(styled_field(
            "state",
            text::clip(&state, value_width),
            label_column(),
            state_style(row.pull_state),
        )),
        field("progress", progress(&row.status)),
    ];
    if let Some(rate) = screen.rate(&row.job, &row.status.progress) {
        let left = rate
            .left_ms
            .map(|left| format!(" · {} left", clock::millis(left)))
            .unwrap_or_default();
        lines.push(field(
            "rate",
            format!("{}/s{left}", text::bytes(rate.bytes_per_second)),
        ));
    }
    if !row.note.is_empty() {
        lines.push(field("note", row.note.clone()));
    }
    lines.push(field("id", row.job.clone()));
    lines.push(field(
        "from",
        format!("{} · {}", row.descriptor.provider, row.reference),
    ));
    lines.push(field_line(
        "to",
        text::elide_middle(&text::at_home(&row.descriptor.destination), value_width),
        label_column(),
    ));
    lines.push(field("started", format!("{} ago", row.started_ago)));
    lines.push(field("updated", format!("{} ago", row.updated_ago)));
    let history = screen.history_lines();
    // The heading and a blank before it cost two rows; the history takes what
    // is left, its newest lines, so the last thing that happened is on screen.
    let room = height.saturating_sub(lines.len() + 2);
    if history.is_empty() || room == 0 {
        return lines;
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(" HISTORY", EYEBROW)));
    let skipped = history.len().saturating_sub(room);
    lines.extend(history.iter().skip(skipped).map(|line| {
        Line::from(Span::styled(
            format!(" {}", text::clip(line, width.saturating_sub(2))),
            DIM,
        ))
    }));
    lines
}

#[cfg(test)]
mod tests;
