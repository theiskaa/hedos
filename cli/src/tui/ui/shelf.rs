//! The shelf table: the `hedos ls` columns with a size instead of a fit
//! verdict, since the verdict only matters when it is not `fits`. A record
//! whose weights are gone is dim and says `gone` where its size would be.

use kernel::profiles::FitVerdict;
use kernel::records::{ModelRecord, ModelState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, BOLD, CAUTION, DIM, SELECTED_ROW, WARM, centered, edited, keys};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::support::shelf_table::{DASH, runtime_label, verdict, verdict_label};
use crate::tui::app::App;
use crate::tui::keymap;
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
    let no_match = shown.is_empty();
    let column_widths = widths(&rows);
    let columns = fitting_columns(&column_widths, area.width);
    let selected = app.selected();

    let body = rows.iter().enumerate().map(|(index, row)| {
        let style = if row.dim() {
            DIM
        } else if row.warm {
            BOLD
        } else {
            Style::new()
        };
        // A row that won't fit is already dim; red is kept for what failed.
        let size_style = match row.verdict {
            Some(FitVerdict::TightFit) => CAUTION,
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
        Row::new(cells).style(style)
    });

    let header = Row::new(columns.iter().map(|&column| match column {
        SIZE => Cell::from(Line::from(HEADERS[SIZE]).right_aligned()),
        _ => Cell::from(HEADERS[column]),
    }))
    .style(DIM);
    let block = Block::bordered().title(title(app)).border_style(DIM);
    let body_area = block.inner(area);
    let table = Table::new(body, constraints(&column_widths, columns))
        .header(header)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(SELECTED_ROW)
        .block(block);
    frame.render_stateful_widget(table, area, &mut app.shelf);
    if no_match {
        draw_no_match(frame, body_area);
    }
}

/// The header stays; the body says why it has no rows.
fn draw_no_match(frame: &mut Frame, body: Rect) {
    let note = Line::from(Span::styled("nothing matches · esc clears the filter", DIM));
    let below_header = Rect {
        y: body.y + 1,
        height: body.height.saturating_sub(1),
        ..body
    };
    let rect = centered(below_header, note.width() as u16, 1);
    frame.render_widget(Paragraph::new(note).centered(), rect);
}

/// One row of the shelf: its cells and the fit verdict they were built from.
struct ShelfRow {
    /// The warm marker, the name, the runtime and store as short labels, and
    /// the size with the verdict when it is not `fits`.
    cells: [String; 5],
    verdict: Option<FitVerdict>,
    warm: bool,
    /// Whether the record's weights are gone from disk.
    gone: bool,
}

impl ShelfRow {
    /// The row for `record`; a record whose weights are gone has no size and
    /// no verdict, only the word.
    fn new(record: &ModelRecord, warm: bool, budget: u64) -> Self {
        let gone = record.state == ModelState::Missing;
        let verdict = if gone {
            None
        } else {
            verdict(record.footprint_mb, budget)
        };
        let mut size = if gone {
            "gone".to_owned()
        } else {
            record
                .footprint_bytes()
                .map_or(DASH.to_owned(), text::bytes)
        };
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
            gone,
        }
    }

    /// Whether the row draws dim: too big for the machine, or gone.
    fn dim(&self) -> bool {
        self.gone || self.verdict == Some(FitVerdict::TooLarge)
    }
}

/// Column widths wide enough for every row and the header; the gutter also
/// holds the selection mark.
fn widths(rows: &[ShelfRow]) -> [usize; 5] {
    let mut widths = HEADERS.map(UnicodeWidthStr::width);
    widths[0] = 2;
    for row in rows {
        for (column, cell) in row.cells.iter().enumerate().skip(1) {
            widths[column] = widths[column].max(cell.width());
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

/// Cells of the title the filter may take while it is typed, mark and
/// cursor included: room for the whole placeholder.
const FILTER_WIDTH: usize = 36;
/// What the filter matches on, shown while it is blank.
const FILTER_PLACEHOLDER: &str = "name, store, runtime, capability";

/// ` shelf `, or the filter as it is typed with how many rows it keeps, plus
/// the sort when it is not the shelf's own order.
fn title(app: &App) -> Line<'static> {
    let mut spans = Vec::new();
    if app.filtering || !app.filter.is_empty() {
        if app.filtering {
            spans.extend(edited(&app.filter, " / ", FILTER_WIDTH, FILTER_PLACEHOLDER));
        } else {
            spans.push(Span::styled(" / ", ACCENT));
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

    let copy = empty_copy(app.facts.memory_bytes);
    let width = KOALA_WIDTH + 2 + copy.iter().map(Line::width).max().unwrap_or(0) as u16;
    let lines: Vec<Line> = KOALA
        .iter()
        .zip(copy)
        .map(|(koala, line)| {
            let mut spans = vec![Span::styled(format!("{koala}  "), BOLD)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect();
    let centered = centered(inner, width, KOALA.len() as u16);
    frame.render_widget(Paragraph::new(lines), centered);
}

/// The copy beside the koala, one line per koala row: what the shelf is,
/// where to start, and the machine's memory as a quiet note.
fn empty_copy(memory_bytes: u64) -> [Line<'static>; 10] {
    let memory = match memory_bytes {
        0 => Line::default(),
        bytes => Line::from(Span::styled(
            format!(" {} GiB on this machine", text::gib(bytes as i64)),
            DIM,
        )),
    };
    [
        Line::default(),
        Line::from(Span::styled(" nothing on the shelf yet", BOLD)),
        Line::default(),
        Line::from(" hedos looks in the Ollama store, the Hugging"),
        Line::from(" Face cache, LM Studio, and loose GGUF or"),
        Line::from(" safetensors files in your folders."),
        Line::default(),
        keys(&[
            ("p", &format!("{} a model", keymap::verb("p"))),
            ("s", &format!("{} again", keymap::verb("s"))),
        ]),
        memory,
        Line::default(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::records::{Capability, Modality, ModelSource, SourceKind};

    use crate::tui::testing::text;

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
    fn a_gone_row_is_dim_and_says_gone() {
        let mut gone = record(Some(16 * 1024));
        gone.state = ModelState::Missing;
        let row = ShelfRow::new(&gone, false, 16 * GIB);
        assert!(row.dim());
        assert_eq!(row.cells[SIZE], "gone");
        assert_eq!(row.verdict, None);
        assert!(!ShelfRow::new(&record(Some(1024)), false, 16 * GIB).dim());
        assert!(ShelfRow::new(&record(Some(16 * 1024)), false, 16 * GIB).dim());
    }

    #[test]
    fn the_empty_shelf_hints_in_the_key_verb_grammar() {
        let copy = empty_copy(64 * GIB);
        let texts: Vec<String> = copy.iter().map(text).collect();
        assert_eq!(texts[7].trim(), "p pull a model  s scan again");
        assert_eq!(texts[8], " 64 GiB on this machine");
        assert_eq!(copy[8].spans[0].style, DIM);
        assert_eq!(text(&empty_copy(0)[8]), "");
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
