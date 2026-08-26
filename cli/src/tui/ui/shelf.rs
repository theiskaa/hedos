//! The shelf table: the `hedos ls` columns with a size instead of a fit
//! verdict, since the verdict only matters when it is not `fits`.

use kernel::profiles::FitVerdict;
use kernel::records::ModelRecord;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use super::{ACCENT, BOLD, CAUTION, CURSOR, DIM, FAILED, WARM};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::support::shelf_table::{DASH, runtime_label, verdict, verdict_label};
use crate::tui::app::App;
use crate::tui::order::Sort;
use crate::tui::text;

/// The column headers, in order: gutter, name, runtime, store, size.
const HEADERS: [&str; 5] = ["", "NAME", "RUNTIME", "STORE", "SIZE"];
/// The column index of the model name, the one that flexes.
const NAME: usize = 1;
/// The column index of the size, the one that is right-aligned.
const SIZE: usize = 4;
/// Space between columns.
const COLUMN_SPACING: u16 = 2;
/// The border on each side of the table.
const CHROME_WIDTH: u16 = 2;
/// Column sets from fullest to sparsest: the store goes first, then the
/// runtime, so a narrow pane keeps the name whole and the size visible.
const COLUMN_SETS: [&[usize]; 3] = [&[0, 1, 2, 3, 4], &[0, 1, 2, 4], &[0, 1, 4]];
/// The gutter mark on the selected row.
const SELECTED: &str = "▎";

/// Draw the shelf into `area`, scrolled so the selection stays in view, or
/// the first-run invitation when there is nothing on it.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.records.is_empty() {
        draw_empty(frame, area, app);
        return;
    }
    let budget = app.facts.memory_bytes;
    let shown: Vec<&ModelRecord> = app.shown().collect();
    let rows: Vec<ShelfRow> = shown
        .iter()
        .map(|record| ShelfRow::new(record, app.facts.is_warm(&record.id), budget))
        .collect();
    let column_widths = widths(&rows);
    let columns = fitting_columns(&column_widths, area.width);
    let selected = app.selected();

    let body = shown
        .iter()
        .zip(&rows)
        .enumerate()
        .map(|(index, (record, row))| {
            let style = match (row.verdict, row.warm) {
                (Some(FitVerdict::TooLarge), _) => DIM,
                (_, true) => BOLD,
                _ => Style::new(),
            };
            let size_style = match row.verdict {
                Some(FitVerdict::TightFit) => CAUTION,
                Some(FitVerdict::TooLarge) => FAILED,
                _ => Style::new(),
            };
            let marker = |mark: &str| {
                let text = format!("{mark}{}", row.cells[0]);
                if row.warm {
                    Cell::from(Span::styled(text, WARM))
                } else {
                    Cell::from(text)
                }
            };
            let cells = columns.iter().map(|&column| match column {
                0 if index == selected => marker(SELECTED),
                0 => marker(" "),
                SIZE => Cell::from(
                    Line::from(Span::styled(row.cells[SIZE].clone(), size_style)).right_aligned(),
                ),
                _ => Cell::from(row.cells[column].clone()),
            });
            let _ = record;
            Row::new(cells).style(style)
        });

    let header = if shown.is_empty() {
        Row::new(vec!["", "nothing matches; esc clears the filter"]).style(DIM)
    } else {
        Row::new(columns.iter().map(|&column| match column {
            SIZE => Cell::from(Line::from(HEADERS[SIZE]).right_aligned()),
            _ => Cell::from(HEADERS[column]),
        }))
        .style(DIM)
    };
    let table = Table::new(body, constraints(&column_widths, columns))
        .header(header)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(title(app)).border_style(DIM));
    frame.render_stateful_widget(table, area, &mut app.shelf);
}

/// One row of the shelf: its cells and the fit verdict they were built from.
struct ShelfRow {
    /// The warm marker, the name, the runtime and store as short labels, and
    /// the size with the verdict when it is not `fits`.
    cells: [String; 5],
    verdict: Option<FitVerdict>,
    warm: bool,
}

impl ShelfRow {
    fn new(record: &ModelRecord, warm: bool, budget: u64) -> Self {
        let verdict = verdict(record.footprint_mb, budget);
        let mut size = record
            .footprint_bytes()
            .map_or(DASH.to_owned(), text::bytes);
        if matches!(verdict, Some(FitVerdict::TightFit | FitVerdict::TooLarge)) {
            size = format!("{size} {}", verdict_label(verdict));
        }
        Self {
            cells: [
                if warm { "●" } else { "○" }.to_owned(),
                record.display_name().to_owned(),
                text::short_runtime(runtime_label(record)).to_owned(),
                text::short_store(record.source.kind.as_str()).to_owned(),
                size,
            ],
            verdict,
            warm,
        }
    }
}

/// Column widths wide enough for every row and the header; the gutter also
/// holds the selection mark.
fn widths(rows: &[ShelfRow]) -> [usize; 5] {
    let mut widths = HEADERS.map(str::len);
    widths[0] = 2;
    for row in rows {
        for (column, cell) in row.cells.iter().enumerate().skip(1) {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    widths
}

/// The fullest column set whose natural widths fit in `width`, or the
/// sparsest when none does.
fn fitting_columns(column_widths: &[usize; 5], width: u16) -> &'static [usize] {
    COLUMN_SETS
        .iter()
        .copied()
        .find(|columns| natural_width(column_widths, columns) <= width as usize)
        .unwrap_or(COLUMN_SETS[COLUMN_SETS.len() - 1])
}

fn natural_width(column_widths: &[usize; 5], columns: &[usize]) -> usize {
    columns
        .iter()
        .map(|&column| column_widths[column])
        .sum::<usize>()
        + COLUMN_SPACING as usize * columns.len().saturating_sub(1)
        + CHROME_WIDTH as usize
}

/// Fixed widths for every column except the name, which takes the rest.
fn constraints(column_widths: &[usize; 5], columns: &[usize]) -> Vec<Constraint> {
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

/// Cells of the filter shown in the title while it is typed.
const FILTER_WIDTH: usize = 24;

/// ` shelf `, or the filter as it is typed with how many rows it keeps, plus
/// the sort when it is not the shelf's own order.
fn title(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    if app.filtering || !app.filter.is_empty() {
        spans.push(Span::styled(" / ", ACCENT));
        if app.filtering {
            let (before, after) = app.filter.view(FILTER_WIDTH);
            spans.push(Span::raw(before));
            spans.push(Span::styled(CURSOR, BOLD));
            spans.push(Span::raw(after));
        } else {
            spans.push(Span::raw(app.filter.as_str().to_owned()));
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

    let memory = match app.facts.memory_bytes {
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
        (format!("p pull a model{memory}"), plain),
        ("s scan again".to_owned(), plain),
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

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    const WIDTHS: [usize; 5] = [2, 20, 10, 8, 7];
    const GIB: u64 = kernel::records::byte_format::BYTES_PER_GIB as u64;

    fn record(footprint_mb: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::huggingface_cache(), "m"),
        );
        record.footprint_mb = footprint_mb;
        record
    }

    #[test]
    fn the_size_cell_carries_the_verdict_only_when_it_matters() {
        assert_eq!(
            ShelfRow::new(&record(Some(1024)), false, 16 * GIB).cells[4],
            "1 GB"
        );
        assert_eq!(
            ShelfRow::new(&record(Some(12 * 1024)), false, 16 * GIB).cells[4],
            "12 GB tight"
        );
        assert_eq!(
            ShelfRow::new(&record(Some(16 * 1024)), false, 16 * GIB).cells[4],
            "16 GB too big"
        );
        assert_eq!(ShelfRow::new(&record(None), false, 16 * GIB).cells[4], DASH);
        assert_eq!(
            ShelfRow::new(&record(Some(1)), true, 16 * GIB).cells[0],
            "●"
        );
        assert_eq!(
            ShelfRow::new(&record(Some(1)), false, 16 * GIB).cells[3],
            "hf"
        );
    }

    #[test]
    fn the_gutter_stays_two_wide() {
        let rows = [ShelfRow::new(&record(Some(1)), true, 16 * GIB)];
        assert_eq!(widths(&rows)[0], 2);
    }

    #[test]
    fn columns_drop_from_the_tail_as_the_pane_narrows() {
        assert_eq!(fitting_columns(&WIDTHS, 200).len(), 5);
        assert_eq!(fitting_columns(&WIDTHS, 50), &[0, 1, 2, 4]);
        assert_eq!(fitting_columns(&WIDTHS, 10), &[0, 1, 4]);
    }

    #[test]
    fn the_name_column_flexes() {
        let constraints = constraints(&WIDTHS, &[0, 1, 4]);
        assert_eq!(constraints[1], Constraint::Fill(1));
        assert_eq!(constraints[2], Constraint::Length(7));
    }
}
