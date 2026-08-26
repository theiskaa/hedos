//! The shelf table, in the same columns `hedos ls` prints.

use kernel::profiles::FitVerdict;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Row, Table};

use super::{BOLD, DIM};
use crate::support::shelf_table::{HEADERS, cells, widths};
use crate::tui::app::App;

/// Space between columns, matching `hedos ls`.
const COLUMN_SPACING: u16 = 2;
/// The border on each side of the table.
const CHROME_WIDTH: u16 = 2;
/// The column index of the model name.
const NAME: usize = 1;
/// Column sets from fullest to sparsest: capabilities go first, then the
/// store, then the runtime, so a narrow pane keeps the name whole and the fit
/// verdict visible.
const COLUMN_SETS: [&[usize]; 4] = [
    &[0, 1, 2, 3, 4, 5],
    &[0, 1, 2, 3, 4],
    &[0, 1, 2, 4],
    &[0, 1, 4],
];

/// Draw the shelf into `area`, scrolled so the selection stays in view.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<[String; 6]> = app
        .records
        .iter()
        .map(|record| {
            cells(
                record,
                app.warm.contains(&record.id),
                app.memory_budget_bytes,
            )
        })
        .collect();
    let column_widths = widths(&rows, Some(&HEADERS));
    let columns = fitting_columns(&column_widths, area.width);

    let body = app.records.iter().zip(&rows).map(|(record, row)| {
        let style = if too_big(record, app.memory_budget_bytes) {
            DIM
        } else if app.warm.contains(&record.id) {
            BOLD
        } else {
            Style::new()
        };
        Row::new(columns.iter().map(|&column| row[column].as_str())).style(style)
    });

    let table = Table::new(body, constraints(&column_widths, columns))
        .header(Row::new(columns.iter().map(|&column| HEADERS[column])).style(DIM))
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(" shelf ").border_style(DIM));
    frame.render_stateful_widget(table, area, &mut app.shelf);
}

fn too_big(record: &kernel::records::ModelRecord, memory_budget_bytes: u64) -> bool {
    FitVerdict::assess(record.footprint_mb, memory_budget_bytes)
        .is_some_and(|fit| fit.verdict == FitVerdict::TooLarge)
}

/// The fullest column set whose natural widths fit in `width`, or the
/// sparsest when none does.
fn fitting_columns(column_widths: &[usize; 6], width: u16) -> &'static [usize] {
    COLUMN_SETS
        .iter()
        .copied()
        .find(|columns| natural_width(column_widths, columns) <= width as usize)
        .unwrap_or(COLUMN_SETS[COLUMN_SETS.len() - 1])
}

fn natural_width(column_widths: &[usize; 6], columns: &[usize]) -> usize {
    columns
        .iter()
        .map(|&column| column_widths[column])
        .sum::<usize>()
        + COLUMN_SPACING as usize * columns.len().saturating_sub(1)
        + CHROME_WIDTH as usize
}

/// Fixed widths for every column except the name, which takes the rest.
fn constraints(column_widths: &[usize; 6], columns: &[usize]) -> Vec<Constraint> {
    columns
        .iter()
        .map(|&column| {
            if column == NAME {
                Constraint::Fill(1)
            } else {
                Constraint::Length(column_widths[column] as u16)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTHS: [usize; 6] = [1, 20, 10, 10, 7, 12];

    #[test]
    fn columns_drop_from_the_tail_as_the_pane_narrows() {
        assert_eq!(fitting_columns(&WIDTHS, 200).len(), 6);
        assert_eq!(fitting_columns(&WIDTHS, 60), &[0, 1, 2, 3, 4]);
        assert_eq!(fitting_columns(&WIDTHS, 48), &[0, 1, 2, 4]);
        assert_eq!(fitting_columns(&WIDTHS, 10), &[0, 1, 4]);
    }

    #[test]
    fn the_name_column_flexes() {
        let constraints = constraints(&WIDTHS, &[0, 1, 4]);
        assert_eq!(constraints[1], Constraint::Fill(1));
        assert_eq!(constraints[2], Constraint::Length(7));
    }
}
