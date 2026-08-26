//! The shelf table, in the same columns `hedos ls` prints.

use kernel::records::ModelRecord;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};

use super::{BOLD, DIM};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::support::shelf_table::{HEADERS, cells, widths};
use crate::tui::app::{App, too_big};
use crate::tui::order::Sort;
use crate::tui::text;

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

/// Draw the shelf into `area`, scrolled so the selection stays in view, or
/// the first-run invitation when there is nothing on it.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.records.is_empty() {
        draw_empty(frame, area, app);
        return;
    }
    let budget = app.memory_budget_bytes();
    let shown: Vec<&ModelRecord> = app.shown().collect();
    let rows: Vec<[String; 6]> = shown
        .iter()
        .map(|record| cells(record, app.facts.is_warm(&record.id), budget))
        .collect();
    let column_widths = widths(&rows, Some(&HEADERS));
    let columns = fitting_columns(&column_widths, area.width);

    let body = shown.iter().zip(&rows).map(|(record, row)| {
        let style = if too_big(record, budget) {
            DIM
        } else if app.facts.is_warm(&record.id) {
            BOLD
        } else {
            Style::new()
        };
        Row::new(columns.iter().map(|&column| row[column].as_str())).style(style)
    });

    let header = if shown.is_empty() {
        Row::new(vec!["", "nothing matches; esc clears the filter"]).style(DIM)
    } else {
        Row::new(columns.iter().map(|&column| HEADERS[column])).style(DIM)
    };
    let table = Table::new(body, constraints(&column_widths, columns))
        .header(header)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(title(app)).border_style(DIM));
    frame.render_stateful_widget(table, area, &mut app.shelf);
}

/// The text cursor shown while the filter is being typed.
const CURSOR: &str = "▏";

/// ` shelf `, or the filter as it is typed with how many rows it keeps, plus
/// the sort when it is not the shelf's own order.
fn title(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    if app.filtering || !app.filter.is_empty() {
        spans.push(Span::styled(" / ", BOLD));
        spans.push(Span::raw(app.filter.clone()));
        if app.filtering {
            spans.push(Span::styled(CURSOR, BOLD));
        }
        spans.push(Span::styled(
            format!(" {} of {} ", app.order.len(), app.records.len()),
            DIM,
        ));
    } else {
        spans.push(Span::raw(" shelf "));
    }
    if app.sort != Sort::Name {
        spans.push(Span::styled(format!("· by {} ", app.sort.label()), DIM));
    }
    Line::from(spans)
}

/// The koala and where to start, for a shelf with nothing on it yet.
fn draw_empty(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" shelf ").border_style(DIM);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let memory = match app.memory_budget_bytes() {
        0 => String::new(),
        bytes => format!("      {} GiB on this machine", text::gib(bytes as i64)),
    };
    let plain = Style::new();
    let copy: [(String, Style); 10] = [
        (String::new(), plain),
        ("nothing on the shelf yet".to_owned(), BOLD),
        (String::new(), plain),
        (
            "hedos looks in the Ollama store, the Hugging".to_owned(),
            plain,
        ),
        ("Face cache, LM Studio, and loose GGUF or".to_owned(), plain),
        ("safetensors files in your folders.".to_owned(), plain),
        (String::new(), plain),
        ("s scan this machine".to_owned(), plain),
        (format!("p pull a model{memory}"), plain),
        (String::new(), plain),
    ];
    let width = KOALA_WIDTH
        + 3
        + copy
            .iter()
            .map(|(line, _)| line.chars().count())
            .max()
            .unwrap_or(0) as u16;
    let lines: Vec<Line> = KOALA
        .iter()
        .zip(copy)
        .map(|(koala, (line, style))| {
            Line::from(vec![
                Span::styled(format!("{koala}   "), BOLD),
                Span::styled(line, style),
            ])
        })
        .collect();
    let [_, centered, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(KOALA.len() as u16),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let [_, centered, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .areas(centered);
    frame.render_widget(Paragraph::new(lines), centered);
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
