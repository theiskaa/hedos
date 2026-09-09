//! The bench screen: the models being measured where the shelf goes, and the
//! selected row's figures where the model's detail goes.
//!
//! The rows are drawn by the same functions `hedos bench` draws with
//! ([`crate::support::bench_view`]), so a row reads the same on either surface;
//! only the frame around them, the selection, and the detail are this screen's.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{
    ACCENT, DIM, EYEBROW, centered, field_line, label_width, pane, selected_row, styled_field,
    value_width,
};
use crate::support::bench_view::{self, DASH, cold_detail, phase, seconds};
use crate::tui::app::App;
use crate::tui::keymap;
use crate::tui::text;
use kernel::bench::{Row, Status, TimingSource};

/// The labels of the detail; the column is as wide as the widest, plus a gap.
const LABELS: [&str; 7] = [
    "state",
    "rate",
    "spread",
    "first token",
    "cold",
    "prompt",
    "timing",
];

/// What the list says before a bench has been started. The key is the bench
/// screen's own, so it is read from the screen's bindings rather than the
/// shelf's, where it is not bound at all.
fn empty_note() -> String {
    format!("no bench yet · a {}", keymap::BENCH.gloss("a"))
}

/// The width of the detail's label column.
fn label_column() -> usize {
    label_width(&LABELS, 1)
}

/// Draw the list of models into `area`, scrolled so the selection stays in
/// view.
pub(super) fn draw_list(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered()
        .title(Span::styled(" bench ", ACCENT))
        .border_style(DIM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = app.bench.rows();
    if rows.is_empty() {
        let note = Line::from(Span::styled(empty_note(), DIM));
        let rect = centered(inner, note.width() as u16, 1);
        frame.render_widget(Paragraph::new(note), rect);
        return;
    }

    let width = inner.width as usize;
    let settled = !app.bench.running();
    let columns = bench_view::columns(rows, width, settled);
    let fastest = kernel::bench::fastest(rows);
    let ordered: Vec<&Row> = if settled {
        kernel::bench::rank(rows)
    } else {
        rows.iter().collect()
    };
    // The selection follows a row, not a position, so ranking the settled
    // table does not move the cursor onto a different model.
    let selected = app.bench.selected_row().map(|row| row.id.as_str());
    let visible = inner.height.saturating_sub(1) as usize;
    let first = scroll(&ordered, selected, visible);

    let mut lines = vec![bench_view::header(&columns)];
    lines.extend(ordered.iter().skip(first).take(visible).map(|row| {
        let line = bench_view::row(row, &columns, fastest, app.ticks());
        if Some(row.id.as_str()) == selected {
            selected_row(line, width)
        } else {
            line
        }
    }));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The first row to draw so the selected one is on screen.
fn scroll(rows: &[&Row], selected: Option<&str>, visible: usize) -> usize {
    if visible == 0 || rows.len() <= visible {
        return 0;
    }
    let index = selected
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .unwrap_or(0);
    index.saturating_sub(visible / 2).min(rows.len() - visible)
}

/// Draw the selected row's figures into `area`.
pub(super) fn draw_detail(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane(" model ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = app.bench.selected_row() else {
        return;
    };
    let labels = label_column();
    let width = value_width(inner.width as usize, labels);
    let mut lines = vec![Line::from(Span::styled(
        format!(" {}", text::clip(&row.name, inner.width as usize)),
        EYEBROW,
    ))];
    lines.extend(detail_lines(row, labels, width));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The selected row's figures, one to a line, and the runs behind them.
fn detail_lines(row: &Row, labels: usize, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    match &row.status {
        Status::Done(figures) => {
            lines.push(field_line("state", "measured", labels));
            let rate = figures.rate().map_or_else(
                || DASH.to_owned(),
                |rate| {
                    let mark = if figures.estimated_tokens { "~" } else { "" };
                    format!("{mark}{rate:.1} tok/s")
                },
            );
            lines.push(field_line("rate", rate, labels));
            if let Some(measure) = figures.tokens_per_second.filter(|m| m.has_spread()) {
                lines.push(field_line(
                    "spread",
                    format!("{:.1} – {:.1}", measure.min, measure.max),
                    labels,
                ));
            }
            if let Some(measure) = figures.ttft_ms {
                lines.push(field_line(
                    "first token",
                    seconds(measure.median as i64),
                    labels,
                ));
            }
            lines.push(field_line("cold", cold_detail(&figures.cold_start), labels));
            if let Some(measure) = figures.prompt_tokens_per_second {
                lines.push(field_line(
                    "prompt",
                    format!("{:.0} tok/s", measure.median),
                    labels,
                ));
            }
            lines.push(field_line("timing", timing(figures.source), labels));
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(" RUNS", EYEBROW)));
            lines.extend(figures.runs.iter().enumerate().map(|(index, sample)| {
                let rate = sample
                    .tokens_per_second()
                    .map_or_else(|| DASH.to_owned(), |rate| format!("{rate:.1} tok/s"));
                Line::from(vec![
                    Span::styled(format!("  {} ", index + 1), DIM),
                    Span::raw(format!(
                        "{rate} · {} tokens · first in {}",
                        sample.completion_tokens,
                        seconds(sample.ttft_ms)
                    )),
                ])
            }));
        }
        Status::Running {
            phase: running,
            tokens,
        } => {
            lines.push(field_line("state", "measuring", labels));
            // The shared phrasing already says "run 2 of 3", so the label is
            // what it is rather than the word again.
            lines.push(field_line("now", phase(*running), labels));
            lines.push(field_line("so far", format!("{tokens} tokens"), labels));
        }
        Status::Waiting => lines.push(field_line("state", "waiting its turn", labels)),
        Status::Stopped => lines.push(field_line("state", "stopped", labels)),
        Status::Failed(reason) | Status::Skipped(reason) => {
            let state = if matches!(row.status, Status::Failed(_)) {
                "failed"
            } else {
                "not measured"
            };
            lines.push(field_line("state", state, labels));
            lines.extend(wrapped_reason(reason, labels, width));
        }
    }
    lines
}

/// The reason a row has no figures, over as many lines as it needs. A pane too
/// narrow to hold a character and its ellipsis shows none of it: `clip` returns
/// nothing there, and a line that takes nothing would never reach the end.
fn wrapped_reason(reason: &str, labels: usize, width: usize) -> Vec<Line<'static>> {
    if width < 2 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut rest = reason;
    let mut label = "why";
    while !rest.is_empty() {
        let taken = text::clip(rest, width);
        // `clip` marks a cut with an ellipsis, which the next line replaces by
        // carrying on from where the text actually ended.
        let kept = taken.trim_end_matches('…');
        if kept.is_empty() {
            break;
        }
        lines.push(Line::from(styled_field(
            label,
            kept.to_owned(),
            labels,
            ratatui::style::Style::new(),
        )));
        rest = &rest[kept.len()..];
        label = "";
    }
    lines
}

/// Where the rate's timing came from, said plainly, since a wall-clock figure
/// and a backend one are not quite the same claim.
fn timing(source: TimingSource) -> &'static str {
    match source {
        TimingSource::Backend => "the runtime's own",
        TimingSource::WallClock => "wall clock",
    }
}
