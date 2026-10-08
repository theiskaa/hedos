//! The shelf table: the `hedos ls` columns with a size instead of a fit
//! verdict, since the verdict only matters when it is not `fits`. A record
//! whose weights are gone is marked in the gutter, drawn dim, and says `gone`
//! where its size would be.

use kernel::profiles::FitVerdict;
use kernel::records::{ModelRecord, ModelState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use unicode_width::UnicodeWidthStr;

use super::card::{Card, scroll_mark};
use super::{
    ACCENT, BOLD, BORDER_COLUMNS, CAUTION, DIM, EYEBROW, SELECTED_MARK, SOFT, card, centered,
    edited, keys, spinner,
};
use crate::support::banner::KOALA_WIDTH;
use crate::support::shelf_table::{marker, runtime_label, verdict, verdict_label};
use crate::support::table::DASH;
use crate::support::text::printable;
use crate::tui::app::App;
use crate::tui::keymap;
use crate::tui::koala;
use crate::tui::layout::Panes;
use crate::tui::motion::Motion;
use crate::tui::palette::{OLIVE, OLIVE_DIM, PAPER, SEL, mix};
use crate::tui::text;

/// The column headers, in order: gutter, name, runtime, store, size.
const HEADERS: [&str; 5] = ["", "NAME", "RUNTIME", "STORE", "SIZE"];
/// The column index of the model name, the one that flexes.
const NAME: usize = 1;
/// The column indices of the runtime and the store, the two read soft.
const RUNTIME: usize = 2;
const STORE: usize = 3;
/// The column index of the size, the one that is right-aligned.
const SIZE: usize = 4;
/// Space between columns.
const COLUMN_SPACING: u16 = 2;
/// Column sets from fullest to sparsest: the store goes first, then the
/// runtime, so a narrow pane keeps the name whole and the size visible.
const COLUMN_SETS: [&[usize]; 3] = [&[0, 1, 2, 3, 4], &[0, 1, 2, 4], &[0, 1, 4]];

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
        .map(|record| {
            ShelfRow::new(record, app.facts.is_warm(&record.id), budget)
                .busy(app.tasks.running_on(&record.id))
        })
        .collect();
    let no_match = shown.is_empty();
    let column_widths = widths(&rows);
    let columns = fitting_columns(&column_widths, area.width);
    let selected = app.selected();

    let marks = Marks {
        motion: &app.motion,
        spin_frame: app.spin_frame(),
    };
    let body = rows
        .iter()
        .enumerate()
        .map(|(index, row)| body_row(row, index == selected, index, columns, &marks));

    let header = Row::new(columns.iter().map(|&column| match column {
        SIZE => Cell::from(Line::from(HEADERS[SIZE]).right_aligned()),
        _ => Cell::from(HEADERS[column]),
    }))
    .style(EYEBROW);
    let total = rows.len();
    Card::new(title(app))
        .right(sort_label(app))
        .render(area, frame.buffer_mut());
    let body_area = Card::inner(area);
    // The selection's tint comes up over a moment when it moves, so the eye
    // follows it.
    let lifted = app.motion.eased(app.selected_at(), SELECTION_FADE_MS);
    let table = Table::new(body, constraints(&column_widths, columns))
        .header(header)
        .column_spacing(COLUMN_SPACING)
        .row_highlight_style(Style::new().bg(mix(PAPER, SEL, lifted)));
    frame.render_stateful_widget(table, body_area, &mut app.shelf);
    let visible = body_area.height.saturating_sub(1) as usize;
    scroll_mark(frame.buffer_mut(), area, app.shelf.offset(), visible, total);
    if no_match {
        draw_no_match(frame, body_area);
    }
}

/// How long the selection's tint takes to come up.
const SELECTION_FADE_MS: u64 = 120;

/// What the state marks need to move: the clock, and the spinner's frame.
struct Marks<'a> {
    motion: &'a Motion,
    spin_frame: u64,
}

impl Marks<'_> {
    /// A warm model's dot, breathing with its own phase so a column of them
    /// never pulses in step.
    fn warm(&self, index: usize) -> Style {
        let breath = self
            .motion
            .age(0)
            .map_or(1.0, |age| 0.5 + 0.5 * (age * 2.1 + index as f32).sin());
        Style::new().fg(mix(OLIVE_DIM, OLIVE, 0.55 + 0.45 * breath))
    }
}

/// One shelf row over `columns`: dim when it can't run here, the name bold
/// when warm or `selected`, the gutter marked when selected, the runtime
/// and store quieter than the name unless the row is dim, and the size in
/// the caution hue on a tight fit. While a task runs on the model, its
/// mark is a spinner.
fn body_row(
    row: &ShelfRow,
    selected: bool,
    index: usize,
    columns: &[usize],
    marks: &Marks,
) -> Row<'static> {
    let style = if row.dim() { DIM } else { Style::new() };
    // A row that won't fit is already dim; red is kept for what failed.
    let size_style = match row.verdict {
        Some(FitVerdict::TightFit) => CAUTION,
        _ => Style::new(),
    };
    let marker = |mark: &'static str| {
        let state = if row.busy {
            Span::styled(format!("{} ", spinner(marks.spin_frame)), ACCENT)
        } else if row.warm {
            Span::styled(row.cells[0].clone(), marks.warm(index))
        } else {
            Span::raw(row.cells[0].clone())
        };
        Cell::from(Line::from(vec![Span::styled(mark, ACCENT), state]))
    };
    let loud = (row.warm || selected) && !row.dim();
    let cells = columns.iter().map(|&column| match column {
        0 if selected => marker(SELECTED_MARK),
        0 => marker(" "),
        NAME if loud => Cell::from(Span::styled(row.cells[NAME].clone(), BOLD)),
        SIZE => Cell::from(
            Line::from(Span::styled(row.cells[SIZE].clone(), size_style)).right_aligned(),
        ),
        RUNTIME | STORE if !row.dim() => Cell::from(Span::styled(row.cells[column].clone(), SOFT)),
        _ => Cell::from(row.cells[column].clone()),
    });
    Row::new(cells).style(style)
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
    /// The state marker, the name, the runtime and store as short labels, and
    /// the size with the verdict when it is not `fits`.
    cells: [String; 5],
    verdict: Option<FitVerdict>,
    warm: bool,
    /// Whether the record's weights are gone from disk.
    gone: bool,
    /// Whether a task is under way on the model: a warm, an unload, a removal.
    busy: bool,
}

impl ShelfRow {
    /// The row for `record`; a record whose weights are gone has no size and
    /// no verdict, only the word.
    fn new(record: &ModelRecord, warm: bool, budget: u64) -> Self {
        let gone = record.state == ModelState::Missing;
        let verdict = if gone {
            None
        } else {
            verdict(record.serving_size(), budget)
        };
        let mut size = if gone {
            "gone".to_owned()
        } else {
            record.serving_size().map_or(DASH.to_owned(), text::bytes)
        };
        if matches!(verdict, Some(FitVerdict::TightFit | FitVerdict::TooLarge)) {
            size = format!("{size} {}", verdict_label(verdict));
        }
        Self {
            cells: [
                marker(record, warm).to_owned(),
                printable(record.display_name()).into_owned(),
                text::short_runtime(runtime_label(record)).to_owned(),
                text::short_store(record.source.kind.as_str()).to_owned(),
                size,
            ],
            verdict,
            warm,
            gone,
            busy: false,
        }
    }

    /// The same row, its mark a spinner while `busy`.
    fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
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

/// The most a name counts for when choosing the columns: past it the name
/// is clipped, so one long repo name never costs every row its runtime and
/// store.
const NAME_SHARE: usize = 28;

fn natural_width(column_widths: &[usize; 5], columns: &[usize]) -> usize {
    columns
        .iter()
        .map(|&column| match column {
            NAME => column_widths[column].min(NAME_SHARE),
            _ => column_widths[column],
        })
        .sum::<usize>()
        + COLUMN_SPACING as usize * columns.len().saturating_sub(1)
        + BORDER_COLUMNS as usize
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

/// `shelf · 15`, or the filter as it is typed with how many rows it keeps.
fn title(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if app.filtering || !app.filter.is_empty() {
        if app.filtering {
            spans.extend(edited(&app.filter, "/ ", FILTER_WIDTH, FILTER_PLACEHOLDER));
        } else {
            spans.push(Span::styled("/ ", ACCENT));
            spans.push(Span::raw(app.filter.as_str().to_owned()));
        }
        spans.push(Span::styled(
            format!(" · {} of {}", app.order.len(), app.records.len()),
            DIM,
        ));
    } else {
        spans.push(Span::styled("shelf", BOLD));
        spans.push(Span::styled(format!(" · {}", app.records.len()), DIM));
    }
    spans
}

/// `by name` on the card's right edge: the order the shelf is in.
fn sort_label(app: &App) -> Vec<Span<'static>> {
    vec![Span::styled(format!("by {}", app.sort.label()), DIM)]
}

/// Where to start, for a shelf with nothing on it yet: the copy, with the
/// koala beside it when the header is one line. With the hero showing, its
/// koala is already on screen, and one living koala is enough.
fn draw_empty(frame: &mut Frame, area: Rect, app: &App) {
    card("shelf").render(area, frame.buffer_mut());
    let inner = Card::inner(area);
    let copy = empty_copy(app.facts.memory_bytes);
    let copy_width = copy.iter().map(Line::width).max().unwrap_or(0) as u16;
    let with_koala = !Panes::hero(frame.area());
    let width = if with_koala {
        KOALA_WIDTH + 2 + copy_width
    } else {
        copy_width
    };
    let rect = centered(inner, width, copy.len() as u16);
    frame.render_widget(
        Paragraph::new(copy.to_vec()),
        Rect {
            x: rect.x + if with_koala { KOALA_WIDTH + 2 } else { 0 },
            width: rect
                .width
                .saturating_sub(if with_koala { KOALA_WIDTH + 2 } else { 0 }),
            ..rect
        },
    );
    if !with_koala {
        return;
    }
    let buf = frame.buffer_mut();
    for (row, cells) in koala::frame(&app.motion).into_iter().enumerate() {
        for (column, cell) in cells.into_iter().enumerate() {
            let (x, y) = (rect.x + column as u16, rect.y + row as u16);
            if let Some(cell) = cell
                && x < inner.right()
                && y < inner.bottom()
            {
                buf[(x, y)].set_char(cell.glyph).set_fg(cell.fg);
            }
        }
    }
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

    /// A Hugging Face chat model `footprint_bytes` large.
    fn sized_record(footprint_bytes: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            "m",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(SourceKind::huggingface_cache(), "m"),
        );
        record.footprint_bytes = footprint_bytes;
        record
    }

    #[test]
    fn the_size_cell_carries_the_verdict_only_when_it_matters() {
        assert_eq!(
            ShelfRow::new(&sized_record(Some(GIB as i64)), false, 16 * GIB).cells[4],
            "1.1 GB"
        );
        assert_eq!(
            ShelfRow::new(&sized_record(Some(12 * GIB as i64)), false, 16 * GIB).cells[4],
            "12.9 GB tight"
        );
        assert_eq!(
            ShelfRow::new(&sized_record(Some(16 * GIB as i64)), false, 16 * GIB).cells[4],
            "17.2 GB too big"
        );
        assert_eq!(
            ShelfRow::new(&sized_record(None), false, 16 * GIB).cells[4],
            DASH
        );
        assert_eq!(
            ShelfRow::new(&sized_record(Some(1)), true, 16 * GIB).cells[0],
            "●"
        );
        assert_eq!(
            ShelfRow::new(&sized_record(Some(1)), false, 16 * GIB).cells[3],
            "hf"
        );
    }

    #[test]
    fn a_multi_quant_row_shows_the_serving_size_without_too_big() {
        let mut record = sized_record(Some(40 * GIB as i64));
        record.serving_bytes = Some(GIB as i64);
        let row = ShelfRow::new(&record, false, 16 * GIB);
        assert_eq!(row.cells[SIZE], "1.1 GB");
        assert_eq!(row.verdict, Some(FitVerdict::RunsWell));
    }

    #[test]
    fn a_name_with_control_or_bidi_characters_shows_them_visibly() {
        let mut record = sized_record(Some(GIB as i64));
        record.name = "evil\u{1b}[31m\u{202e}gpj\nx\u{2028}y".to_owned();
        let name = &ShelfRow::new(&record, false, 16 * GIB).cells[1];
        assert_eq!(name, "evil\\u{1b}[31m\\u{202e}gpj\\nx\\u{2028}y");
        assert!(name.chars().all(|c| !c.is_control()));
    }

    #[test]
    fn a_gone_row_is_dim_and_says_gone() {
        let mut gone = sized_record(Some(16 * GIB as i64));
        gone.state = ModelState::Missing;
        let row = ShelfRow::new(&gone, false, 16 * GIB);
        assert!(row.dim());
        assert_eq!(row.cells[SIZE], "gone");
        assert_eq!(row.cells[0], "✕", "and the gutter marks it");
        assert_eq!(row.verdict, None);
        assert!(!ShelfRow::new(&sized_record(Some(GIB as i64)), false, 16 * GIB).dim());
        assert!(ShelfRow::new(&sized_record(Some(16 * GIB as i64)), false, 16 * GIB).dim());
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
        let rows = [ShelfRow::new(&sized_record(Some(1 << 20)), true, 16 * GIB)];
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
