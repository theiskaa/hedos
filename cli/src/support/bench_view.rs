//! Drawing a bench: the block that redraws in place while models run, the
//! table it settles into, and the plain text a pipe gets.
//!
//! One set of row cells feeds all three, and the shelf's bench screen draws
//! through the same functions, so a row reads the same wherever it appears.

use kernel::bench::{self, ColdStart, Phase, Row, Status};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use runtime::bench::BenchEvent;
use unicode_width::UnicodeWidthStr;

use crate::support::table;
use crate::support::text;
use crate::tui::palette::{
    ACCENT, BAR_EMPTY, BAR_FILLED, BOLD, CAUTION, COOL, DIM, EYEBROW, FAILED, spinner,
};

/// The placeholder for a figure a row does not have.
pub(crate) const DASH: &str = "—";
/// The rate bar at its widest, and the width it narrows to before it goes.
const BAR_WIDE: usize = 20;
const BAR_NARROW: usize = 10;
/// The fewest cells a name keeps once everything else has been shed.
const NAME_MIN: usize = 12;
/// Cells the rate figure and its spread take.
const RATE_WIDTH: usize = 5;
const SPREAD_WIDTH: usize = 7;
/// Cells a time figure takes: `0.18s`, `12.4s`.
const TIME_WIDTH: usize = 6;
/// Cells between columns.
const GAP: usize = 2;
/// The rows a block spends above its model rows (the title, a blank, the
/// column header) and below them (a blank and the machine line).
pub(crate) const HEADER_ROWS: usize = 3;
pub(crate) const FOOTER_ROWS: usize = 2;
pub(crate) const CHROME_ROWS: usize = HEADER_ROWS + FOOTER_ROWS;

/// Which columns fit the terminal, and how wide the flexible ones are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Columns {
    /// Cells the name column takes.
    pub name: usize,
    /// Cells the runtime column takes; zero drops it.
    pub runtime: usize,
    /// Cells the quantization column takes; zero drops it.
    pub quant: usize,
    /// Cells the rate bar takes; zero drops the bar and keeps the figure.
    pub bar: usize,
    /// Whether the spread is printed beside the rate.
    pub spread: bool,
    /// Whether the time-to-first-token column is drawn.
    pub ttft: bool,
    /// Whether the cold-start column is drawn.
    pub cold: bool,
    /// The cells the whole row has, so text that stands in for the figures (a
    /// reason, a phase) can be cut to what is left rather than overrunning the
    /// pane it is drawn in.
    pub line: usize,
}

/// The columns that fit `width` cells, shedding in the order a reader can most
/// afford to lose: the cold start first, then the wait, then the spread, then
/// the bar narrows and goes, then the quantization and the runtime together,
/// and the name clips last. `settled` asks for the spread, which only the
/// finished table shows.
pub(crate) fn columns(rows: &[Row], width: usize, settled: bool) -> Columns {
    let widest = |cell: fn(&Row) -> Option<&String>| {
        rows.iter()
            .filter_map(cell)
            .map(|text| text.width())
            .max()
            .unwrap_or(0)
    };
    let mut columns = Columns {
        name: rows
            .iter()
            .map(|row| row.name.width())
            .max()
            .unwrap_or(0)
            .max("NAME".width()),
        runtime: widest(|row| row.runtime.as_ref()).max("RUNTIME".width()),
        quant: widest(|row| row.quantization.as_ref()).max("QUANT".width()),
        bar: BAR_WIDE,
        spread: settled,
        ttft: true,
        cold: true,
        line: width,
    };
    let steps: [fn(&mut Columns); 6] = [
        |columns| columns.cold = false,
        |columns| columns.ttft = false,
        |columns| columns.spread = false,
        |columns| columns.bar = BAR_NARROW,
        |columns| columns.bar = 0,
        |columns| {
            columns.quant = 0;
            columns.runtime = 0;
        },
    ];
    for step in steps {
        if line_width(&columns) <= width {
            return columns;
        }
        step(&mut columns);
    }
    if line_width(&columns) > width {
        // Nothing left to shed: the name takes what is left, down to a stub.
        let others = line_width(&columns) - columns.name;
        columns.name = width.saturating_sub(others).max(NAME_MIN);
    }
    columns
}

/// Cells a row spends before its figures: the gutter, the name, and the two
/// columns that may sit between.
fn prefix_width(columns: &Columns) -> usize {
    let mut width = 2 + columns.name + GAP;
    for column in [columns.runtime, columns.quant] {
        if column > 0 {
            width += column + GAP;
        }
    }
    width
}

/// Cells a row of `columns` draws in, gutter and gaps included.
fn line_width(columns: &Columns) -> usize {
    let mut width = 2 + columns.name;
    for column in [columns.runtime, columns.quant] {
        if column > 0 {
            width += GAP + column;
        }
    }
    width += GAP + columns.bar + usize::from(columns.bar > 0) * GAP + RATE_WIDTH;
    if columns.spread {
        width += GAP + SPREAD_WIDTH;
    }
    for shown in [columns.ttft, columns.cold] {
        if shown {
            width += GAP + TIME_WIDTH;
        }
    }
    width
}

/// The header over the rows.
pub(crate) fn header(columns: &Columns) -> Line<'static> {
    let mut spans = vec![Span::raw("  ")];
    let mut cell = |text: &str, width: usize| {
        spans.push(Span::styled(padded(text, width), EYEBROW));
        spans.push(Span::raw(" ".repeat(GAP)));
    };
    cell("NAME", columns.name);
    if columns.runtime > 0 {
        cell("RUNTIME", columns.runtime);
    }
    if columns.quant > 0 {
        cell("QUANT", columns.quant);
    }
    cell(
        "TOK/S",
        columns.bar + usize::from(columns.bar > 0) * GAP + RATE_WIDTH,
    );
    if columns.spread {
        cell("", SPREAD_WIDTH);
    }
    if columns.ttft {
        cell("TTFT", TIME_WIDTH);
    }
    if columns.cold {
        cell("COLD", TIME_WIDTH);
    }
    Line::from(trimmed(spans))
}

/// One row, drawn against the `fastest` rate any row measured. `ticks` turns
/// the spinner on the row that is running.
pub(crate) fn row(row: &Row, columns: &Columns, fastest: Option<f64>, ticks: u64) -> Line<'static> {
    let mut spans = vec![gutter(row, ticks), Span::raw(" ")];
    spans.push(Span::styled(
        padded(&text::clip(&row.name, columns.name), columns.name),
        name_style(row),
    ));
    spans.push(Span::raw(" ".repeat(GAP)));
    for (width, value) in [
        (columns.runtime, row.runtime.as_deref()),
        (columns.quant, row.quantization.as_deref()),
    ] {
        if width > 0 {
            spans.push(Span::styled(padded(value.unwrap_or(DASH), width), COOL));
            spans.push(Span::raw(" ".repeat(GAP)));
        }
    }
    spans.extend(figures(row, columns, fastest));
    Line::from(trimmed(spans))
}

/// The mark in front of a row: the spinner while it runs, else a space.
fn gutter(row: &Row, ticks: u64) -> Span<'static> {
    match row.status {
        Status::Running { .. } => Span::styled(spinner(ticks).to_owned(), ACCENT),
        _ => Span::raw(" "),
    }
}

/// A row that measured something, or is measuring, is loud; one that produced
/// nothing is quiet.
fn name_style(row: &Row) -> Style {
    match row.status {
        Status::Done(_) | Status::Running { .. } => BOLD,
        _ => DIM,
    }
}

/// What a row says where its figures go: the bar and the rate for a measured
/// one, the phase for a running one, and the reason for everything else. A bar
/// means one thing only, the rate, so only a measured row draws one.
fn figures(row: &Row, columns: &Columns, fastest: Option<f64>) -> Vec<Span<'static>> {
    // Whatever stands in for the figures is one free-text cell, so it is cut to
    // the room the columns before it left rather than running past the edge.
    let room = columns.line.saturating_sub(prefix_width(columns));
    let note = |text: String, style| vec![Span::styled(text::clip(&text, room), style)];
    let figures = match &row.status {
        Status::Done(figures) => figures,
        Status::Running { phase, tokens } => return note(running(*phase, *tokens), ACCENT),
        Status::Waiting => return note("waiting".to_owned(), DIM),
        Status::Failed(reason) => return note(reason.clone(), FAILED),
        Status::Skipped(reason) => return note(reason.clone(), DIM),
        Status::Stopped => return note("stopped".to_owned(), DIM),
    };
    let Some(rate) = figures.rate() else {
        return note("no figure".to_owned(), DIM);
    };

    let mut spans = Vec::new();
    if columns.bar > 0 {
        let (lit, unlit) = bar_text(rate, fastest, columns.bar);
        spans.push(Span::styled(lit, ACCENT));
        spans.push(Span::styled(unlit, DIM));
        spans.push(Span::raw(" ".repeat(GAP)));
    }
    spans.push(rate_span(rate, figures.estimated_tokens));
    if columns.spread {
        spans.push(Span::raw(" ".repeat(GAP)));
        spans.push(Span::styled(padded(&spread(row), SPREAD_WIDTH), DIM));
    }
    if columns.ttft {
        let median = figures.ttft_ms.map(|measure| measure.median as i64);
        spans.push(Span::raw(" ".repeat(GAP)));
        spans.push(Span::styled(
            padded(&optional_seconds(median), TIME_WIDTH),
            DIM,
        ));
    }
    if columns.cold {
        spans.push(Span::raw(" ".repeat(GAP)));
        spans.push(Span::styled(
            padded(&cold(&figures.cold_start), TIME_WIDTH),
            DIM,
        ));
    }
    spans
}

/// The rate as both surfaces write it, worn with a `~` when the tokens behind
/// it were counted from the text rather than reported.
fn rate_text(rate: f64, estimated: bool) -> String {
    let mark = if estimated { "~" } else { "" };
    format!("{mark}{rate:.1}")
}

/// The bar as both surfaces draw it: `filled` cells of `width` lit.
fn bar_text(rate: f64, fastest: Option<f64>, width: usize) -> (String, String) {
    let filled = bench::filled_cells(rate, fastest.unwrap_or(rate), width);
    (BAR_FILLED.repeat(filled), BAR_EMPTY.repeat(width - filled))
}

/// The rate in the drawn table, where the estimate mark also takes a hue.
fn rate_span(rate: f64, estimated: bool) -> Span<'static> {
    let figure = right_aligned(&rate_text(rate, estimated), RATE_WIDTH);
    if estimated {
        Span::styled(figure, CAUTION)
    } else {
        Span::styled(figure, BOLD)
    }
}

/// `run 2 of 3`, or `cold start` before the warm runs.
pub(crate) fn phase(phase: Phase) -> String {
    match phase {
        Phase::ColdStart => "cold start".to_owned(),
        Phase::Warm { run, of } => format!("run {run} of {of}"),
    }
}

/// The phase with what it has produced so far, for a row in a table.
fn running(running: Phase, tokens: i64) -> String {
    let phase = phase(running);
    if tokens > 0 {
        format!("{phase} · {tokens} tokens")
    } else {
        phase
    }
}

/// `66–71`, or nothing when the runs landed on the same figure.
fn spread(row: &Row) -> String {
    match row
        .status
        .figures()
        .and_then(|figures| figures.tokens_per_second)
    {
        Some(measure) if measure.has_spread() => {
            format!("{:.0}–{:.0}", measure.min, measure.max)
        }
        _ => String::new(),
    }
}

/// The cold column, which has one cell to say it in: the figure, `held` for a
/// model something else keeps in memory, and a dash where no cold run was made.
fn cold(cold: &ColdStart) -> String {
    match cold {
        ColdStart::Measured(ms) => seconds(*ms),
        ColdStart::Held(_) => "held".to_owned(),
        ColdStart::NotMeasured => DASH.to_owned(),
    }
}

/// The same fact where there is room for the whole of it, as the shelf's
/// detail pane has: who holds the model, rather than only that someone does.
pub(crate) fn cold_detail(cold: &ColdStart) -> String {
    match cold {
        ColdStart::Measured(ms) => seconds(*ms),
        ColdStart::Held(holder) => format!("held by {holder}"),
        ColdStart::NotMeasured => "not measured".to_owned(),
    }
}

/// A span in milliseconds: `0.18s` under a second, `1.4s` over one.
pub(crate) fn seconds(ms: i64) -> String {
    let seconds = ms as f64 / 1000.0;
    if seconds < 1.0 {
        format!("{seconds:.2}s")
    } else {
        format!("{seconds:.1}s")
    }
}

fn optional_seconds(ms: Option<i64>) -> String {
    ms.map_or_else(|| DASH.to_owned(), seconds)
}

fn padded(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(pad))
}

fn right_aligned(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.width());
    format!("{}{text}", " ".repeat(pad))
}

/// Drop the padding a row ends on, so no line carries a tail of styled spaces
/// past its last cell.
fn trimmed(mut spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    while spans
        .last()
        .is_some_and(|span| span.content.trim().is_empty())
    {
        spans.pop();
    }
    spans
}

/// A bench being watched: its rows, the plan they were measured under, and the
/// machine they belong to. The command and the shelf both keep one.
#[derive(Debug, Clone)]
pub(crate) struct Board {
    /// The rows, in the order the bench walks them.
    pub(crate) rows: Vec<Row>,
    /// Warm runs per model.
    pub(crate) runs: usize,
    /// The cap on each reply.
    pub(crate) max_tokens: i64,
    /// The machine line under the table.
    pub(crate) machine: String,
}

impl Board {
    /// A board over `rows`, measured `runs` times at `max_tokens` each, on
    /// `machine`.
    pub(crate) fn new(rows: Vec<Row>, runs: usize, max_tokens: i64, machine: String) -> Self {
        Self {
            rows,
            runs,
            max_tokens,
            machine,
        }
    }

    /// Fold in one step of the bench; whether anything the board shows moved.
    pub(crate) fn apply(&mut self, event: &BenchEvent) -> bool {
        let (id, status) = match event {
            BenchEvent::Started { id, phase } => (
                id,
                Status::Running {
                    phase: *phase,
                    tokens: 0,
                },
            ),
            BenchEvent::Tokens { id, tokens } => {
                let Some(row) = self.row_mut(id) else {
                    return false;
                };
                // Only a row that is running has somewhere to put a count; one
                // for a run that has just ended is a message in flight.
                let Status::Running { phase, .. } = row.status else {
                    return false;
                };
                row.status = Status::Running {
                    phase,
                    tokens: *tokens,
                };
                return true;
            }
            BenchEvent::Settled { id, status } => (id, (**status).clone()),
        };
        match self.row_mut(id) {
            Some(row) => {
                row.status = status;
                true
            }
            None => false,
        }
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut Row> {
        self.rows.iter_mut().find(|row| row.id == id)
    }

    /// Whether every row has been settled one way or another.
    pub(crate) fn finished(&self) -> bool {
        !self
            .rows
            .iter()
            .any(|row| matches!(row.status, Status::Waiting | Status::Running { .. }))
    }

    /// How many of the rows the bench actually walks have finished, and how
    /// many there are. A row skipped before anything ran is not progress and
    /// is not counted at either end.
    pub(crate) fn progress(&self) -> (usize, usize) {
        let walked = self
            .rows
            .iter()
            .filter(|row| !matches!(row.status, Status::Skipped(_)));
        let total = walked.clone().count();
        let done = walked
            .filter(|row| !matches!(row.status, Status::Waiting | Status::Running { .. }))
            .count();
        (done, total)
    }

    /// The index of the row being measured, for keeping it in view.
    pub(crate) fn running(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row.status, Status::Running { .. }))
    }

    /// Rows the whole block needs, chrome included.
    pub(crate) fn height(&self) -> u16 {
        let rows = u16::try_from(self.rows.len()).unwrap_or(u16::MAX);
        rows.saturating_add(CHROME_ROWS as u16)
    }

    /// The whole block: the title, the column header, the rows, and the
    /// machine under them. `settled` ranks the rows fastest first and adds the
    /// spread, which is what the finished table shows.
    pub(crate) fn lines(&self, width: usize, ticks: u64, settled: bool) -> Vec<Line<'static>> {
        let columns = columns(&self.rows, width, settled);
        let fastest = bench::fastest(&self.rows);
        let ordered: Vec<&Row> = if settled {
            bench::rank(&self.rows)
        } else {
            self.rows.iter().collect()
        };
        let mut lines = vec![self.title(), Line::default(), header(&columns)];
        lines.extend(
            ordered
                .into_iter()
                .map(|entry| row(entry, &columns, fastest, ticks)),
        );
        lines.push(Line::default());
        lines.push(self.footer(settled));
        lines
    }

    /// ` bench · 6 models · 3 warm runs after a cold one · 128 tokens`.
    fn title(&self) -> Line<'static> {
        let runs = match self.runs {
            1 => "1 warm run after a cold one".to_owned(),
            runs => format!("{runs} warm runs after a cold one"),
        };
        Line::from(vec![
            Span::styled(" bench".to_owned(), ACCENT),
            Span::styled(
                format!(
                    "  ·  {}  ·  {runs}  ·  {} tokens",
                    text::count(self.rows.len(), "model"),
                    self.max_tokens
                ),
                DIM,
            ),
        ])
    }

    /// The machine, then how far along the bench is, or the note that says why
    /// a figure wears a `~`.
    fn footer(&self, settled: bool) -> Line<'static> {
        let mut spans = vec![Span::styled(format!(" {}", self.machine), DIM)];
        let tail = if settled {
            self.rows
                .iter()
                .any(|row| {
                    row.status
                        .figures()
                        .is_some_and(|figures| figures.estimated_tokens)
                })
                .then(|| "~ counted from the text, the runtime reports no token count".to_owned())
        } else {
            {
                let (done, total) = self.progress();
                Some(format!("{done} of {total} done"))
            }
        };
        if let Some(tail) = tail {
            spans.push(Span::styled(format!("  ·  {tail}"), DIM));
        }
        Line::from(spans)
    }
}

/// The block cut to `height` rows: the title, header and footer are kept and
/// the model rows scroll, so the row being measured stays in view.
pub(crate) fn windowed(
    lines: Vec<Line<'static>>,
    height: usize,
    focus: Option<usize>,
) -> Vec<Line<'static>> {
    if lines.len() <= height || height <= CHROME_ROWS {
        return lines;
    }
    let room = height - CHROME_ROWS;
    let rows = &lines[HEADER_ROWS..lines.len() - FOOTER_ROWS];
    // Centre the running row in what is left, clamped to the ends of the list.
    let first = focus
        .unwrap_or(0)
        .saturating_sub(room / 2)
        .min(rows.len() - room);
    let mut kept = lines[..HEADER_ROWS].to_vec();
    kept.extend_from_slice(&rows[first..first + room]);
    kept.extend_from_slice(&lines[lines.len() - FOOTER_ROWS..]);
    kept
}

/// The plain table a pipe gets: the settled columns, ranked, without styling.
pub(crate) fn plain(rows: &[Row]) -> String {
    let fastest = bench::fastest(rows);
    let headers = [
        "", "NAME", "RUNTIME", "QUANT", "TOK/S", "SPREAD", "TTFT", "COLD",
    ];
    let cells: Vec<Vec<String>> = bench::rank(rows)
        .into_iter()
        .map(|row| plain_cells(row, fastest))
        .collect();
    table::render(&headers, &cells)
}

fn plain_cells(row: &Row, fastest: Option<f64>) -> Vec<String> {
    let note = |text: String| (text, String::new(), DASH.to_owned(), DASH.to_owned());
    let (rate, spread_cell, ttft, cold_cell) = match &row.status {
        Status::Done(figures) => match figures.rate() {
            Some(rate) => {
                let (lit, unlit) = bar_text(rate, fastest, BAR_WIDE);
                (
                    format!(
                        "{lit}{unlit}  {}",
                        rate_text(rate, figures.estimated_tokens)
                    ),
                    spread(row),
                    optional_seconds(figures.ttft_ms.map(|measure| measure.median as i64)),
                    cold(&figures.cold_start),
                )
            }
            None => note("no figure".to_owned()),
        },
        Status::Failed(reason) | Status::Skipped(reason) => note(reason.clone()),
        Status::Stopped => note("stopped".to_owned()),
        Status::Waiting | Status::Running { .. } => note("not run".to_owned()),
    };
    vec![
        String::new(),
        row.name.clone(),
        row.runtime.clone().unwrap_or_else(|| DASH.to_owned()),
        row.quantization.clone().unwrap_or_else(|| DASH.to_owned()),
        rate,
        spread_cell,
        ttft,
        cold_cell,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::bench::{ColdStart, Figures, Sample, WallClock};
    use kernel::capabilities::GenerationStats;

    fn measured(id: &str, tokens: i64, decode_ms: i64) -> Row {
        let stats = GenerationStats {
            completion_tokens: Some(tokens),
            eval_ms: Some(decode_ms),
            ..GenerationStats::default()
        };
        let sample = Sample::new(
            Some(&stats),
            "",
            WallClock {
                ttft_ms: 200,
                total_ms: 200 + decode_ms,
            },
        );
        let mut row = Row::waiting(id, id, Some("ollama".to_owned()), Some("Q4_K_M".to_owned()));
        row.status = Status::Done(Box::new(Figures::summarize(
            ColdStart::Measured(900),
            vec![sample],
        )));
        row
    }

    fn board(rows: Vec<Row>) -> Board {
        Board::new(rows, 3, 128, "apple m3 max · 36 GiB".to_owned())
    }

    #[test]
    fn a_wide_terminal_keeps_every_column_and_a_narrow_one_sheds_them_in_order() {
        let rows = vec![measured("gemma3", 60, 1000)];
        let wide = columns(&rows, 120, true);
        assert!(wide.cold && wide.ttft && wide.spread);
        assert_eq!(wide.bar, BAR_WIDE);

        // The cold start goes first, then the wait, then the spread.
        assert!(!columns(&rows, 74, true).cold);
        let tight = columns(&rows, 66, true);
        assert!(!tight.ttft && !tight.cold);
        let tighter = columns(&rows, 40, true);
        assert!(!tighter.spread);
        assert!(tighter.bar <= BAR_NARROW);
    }

    #[test]
    fn a_name_is_never_shed_below_a_readable_stub() {
        let rows = vec![measured("a-very-long-model-name-indeed", 60, 1000)];
        assert!(columns(&rows, 10, false).name >= NAME_MIN);
    }

    #[test]
    fn only_a_measured_row_draws_a_bar() {
        let mut waiting = Row::waiting("w", "w", None, None);
        waiting.status = Status::Waiting;
        let rows = vec![measured("fast", 60, 1000), waiting];
        let columns = columns(&rows, 120, false);
        let fastest = bench::fastest(&rows);
        let drawn = row(&rows[1], &columns, fastest, 0);
        let text: String = drawn
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("waiting"));
        assert!(!text.contains(BAR_FILLED), "{text}");
    }

    #[test]
    fn an_estimated_rate_wears_a_tilde() {
        let mut row_with = measured("m", 60, 1000);
        if let Status::Done(figures) = &mut row_with.status {
            figures.estimated_tokens = true;
        }
        let columns = columns(std::slice::from_ref(&row_with), 120, false);
        let drawn = row(&row_with, &columns, Some(60.0), 0);
        let text: String = drawn
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("~60.0"), "{text}");
    }

    #[test]
    fn a_long_reason_is_cut_to_the_room_the_columns_left() {
        let rows = vec![Row::skipped(
            "big",
            "big",
            Some("ollama".to_owned()),
            Some("Q4_K_M".to_owned()),
            "no runtime serves it, and this reason runs on well past the edge",
        )];
        let columns = columns(&rows, 60, false);
        let drawn = row(&rows[0], &columns, None, 0);
        assert!(drawn.width() <= 60, "{} cells", drawn.width());
    }

    #[test]
    fn the_settled_block_ranks_fastest_first_and_the_live_one_keeps_shelf_order() {
        let board = board(vec![measured("slow", 10, 1000), measured("fast", 60, 1000)]);
        let settled = board.lines(120, 0, true);
        let live = board.lines(120, 0, false);
        let first = |lines: &[Line<'static>]| {
            lines[3]
                .spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        assert!(first(&settled).contains("fast"));
        assert!(first(&live).contains("slow"));
    }

    #[test]
    fn a_tokens_event_only_lands_on_a_row_that_is_running() {
        let mut board = board(vec![Row::waiting("m", "m", None, None)]);
        assert!(!board.apply(&BenchEvent::Tokens {
            id: "m".to_owned(),
            tokens: 12,
        }));
        assert!(board.apply(&BenchEvent::Started {
            id: "m".to_owned(),
            phase: Phase::ColdStart,
        }));
        assert!(board.apply(&BenchEvent::Tokens {
            id: "m".to_owned(),
            tokens: 12,
        }));
        assert_eq!(
            board.rows[0].status,
            Status::Running {
                phase: Phase::ColdStart,
                tokens: 12
            }
        );
    }

    #[test]
    fn a_board_is_finished_when_every_row_has_a_figure_or_a_reason() {
        let mut board = board(vec![
            Row::waiting("a", "a", None, None),
            Row::skipped("b", "b", None, None, "too big"),
        ]);
        assert!(!board.finished());
        assert_eq!(
            board.progress(),
            (0, 1),
            "the skipped row is not one the bench walks"
        );
        board.apply(&BenchEvent::Settled {
            id: "a".to_owned(),
            status: Box::new(Status::Stopped),
        });
        assert!(board.finished());
    }

    #[test]
    fn the_footer_explains_a_tilde_only_when_the_table_has_one() {
        let mut estimated = measured("m", 60, 1000);
        if let Status::Done(figures) = &mut estimated.status {
            figures.estimated_tokens = true;
        }
        let plain_footer = |board: &Board| {
            board
                .lines(120, 0, true)
                .last()
                .expect("a footer")
                .spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        assert!(plain_footer(&board(vec![estimated])).contains("counted from the text"));
        assert!(!plain_footer(&board(vec![measured("m", 60, 1000)])).contains("counted from"));
    }

    fn numbered(count: usize) -> Vec<Line<'static>> {
        (0..count)
            .map(|index| Line::from(Span::raw(index.to_string())))
            .collect()
    }

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_block_that_fits_is_left_alone() {
        let block = numbered(8);
        assert_eq!(windowed(block.clone(), 10, Some(2)).len(), block.len());
    }

    #[test]
    fn a_long_block_keeps_its_chrome_and_scrolls_to_the_running_row() {
        // 3 chrome lines, 10 rows, 2 chrome lines.
        let block = numbered(15);
        let kept = windowed(block, 9, Some(9));
        let text = texts(&kept);
        assert_eq!(kept.len(), 9);
        assert_eq!(&text[..3], &["0", "1", "2"], "the title and header stay");
        assert_eq!(&text[7..], &["13", "14"], "and so does the footer");
        assert!(
            text.contains(&"12".to_owned()),
            "the running row is in view: {text:?}"
        );
    }

    #[test]
    fn the_window_never_runs_past_the_end_of_the_list() {
        let kept = windowed(numbered(15), 9, Some(0));
        assert_eq!(kept.len(), 9);
        assert_eq!(texts(&kept)[3], "3", "it starts at the first row");
    }

    #[test]
    fn the_plain_table_carries_the_reason_a_row_has_no_figure() {
        let rows = vec![
            measured("fast", 60, 1000),
            Row::skipped("big", "big", None, None, "too big for 36 GiB"),
        ];
        let rendered = plain(&rows);
        assert!(rendered.contains("TOK/S"));
        assert!(rendered.contains("too big for 36 GiB"), "{rendered}");
        // Ranked, so the measured row comes before the one that never ran.
        let fast = rendered.find("fast").expect("the measured row");
        let big = rendered.find("big").expect("the skipped row");
        assert!(fast < big);
    }
}
