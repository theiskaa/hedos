//! The machine block under the shelf: what memory holds, what disk holds,
//! and the gateway beside it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{BOLD, DIM, field};
use crate::tui::app::App;
use crate::tui::facts::Facts;
use crate::tui::text;

const LABEL_WIDTH: usize = 7;
/// The three brightness steps the memory bar cycles through, one per resident.
const SEGMENT_STYLES: [Style; 3] = [BOLD, Style::new(), DIM];
/// Cells the memory figure to the right of the bar needs: `  14.2 of 64 GiB`.
const FIGURE_WIDTH: u16 = 18;
const MIN_BAR_WIDTH: u16 = 10;

/// How many lines the machine block needs: memory and disk, plus the legend
/// when something is loaded.
pub(super) fn lines(facts: &Facts) -> u16 {
    if facts.residents.is_empty() { 2 } else { 3 }
}

/// Draw the machine block into `machine` and the gateway block into
/// `gateway`; each is skipped when its rect has no room.
pub(super) fn draw(frame: &mut Frame, machine: Rect, gateway: Rect, app: &App) {
    if machine.height > 0 {
        draw_machine(frame, machine, &app.facts);
    }
    if gateway.height > 0 && gateway.width > 0 {
        draw_gateway(frame, gateway, &app.facts);
    }
}

fn draw_machine(frame: &mut Frame, area: Rect, facts: &Facts) {
    let block = Block::bordered().title(" machine ").border_style(DIM);
    let inner = block.inner(area);
    let bar_width = inner
        .width
        .saturating_sub(LABEL_WIDTH as u16 + 1 + FIGURE_WIDTH)
        .max(MIN_BAR_WIDTH) as usize;
    let lines = if facts.residents.is_empty() {
        let mut spans = field("memory", "", LABEL_WIDTH);
        spans.push(Span::styled("nothing loaded", DIM));
        spans.push(Span::raw(" · "));
        spans.extend(free_of_total(facts));
        vec![Line::from(spans), disk_line(facts)]
    } else {
        vec![
            memory_line(facts, bar_width),
            legend_line(facts),
            disk_line(facts),
        ]
    };
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_gateway(frame: &mut Frame, area: Rect, facts: &Facts) {
    let block = Block::bordered().title(" gateway ").border_style(DIM);
    let inner = block.inner(area);
    let state = match facts.gateway_port {
        Some(_) => Span::styled(format!(" {}", gateway_state(facts)), BOLD),
        None => Span::styled(" off · hedos serve to start", DIM),
    };
    let lines = vec![
        Line::from(state),
        Line::from(Span::styled(format!(" {}", served_line(facts)), DIM)),
    ];
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `on :11434 · 3 req/min`, or `off`.
pub(super) fn gateway_state(facts: &Facts) -> String {
    match facts.gateway_port {
        Some(port) => format!(
            "on :{port} · {} req/min",
            facts.activity.requests_last_minute
        ),
        None => "off".to_owned(),
    }
}

/// `54.7` bold, then ` GiB free of 64` dim.
pub(super) fn free_of_total(facts: &Facts) -> [Span<'static>; 2] {
    [
        Span::styled(text::gib(facts.free_bytes()), BOLD),
        Span::styled(
            format!(" GiB free of {}", text::gib(facts.memory_bytes as i64)),
            DIM,
        ),
    ]
}

/// `last request 21d ago · 87 all time`, or a quiet note when the log is empty.
fn served_line(facts: &Facts) -> String {
    let activity = &facts.activity;
    if activity.total_requests == 0 {
        return "nothing served yet".to_owned();
    }
    format!(
        "last request {} ago · {} all time",
        text::duration((kernel::time::now_millis() - activity.last_request_millis) / 1000),
        text::count(activity.total_requests as usize, "request")
    )
}

/// `memory  ████░░░░  14.2 of 64 GiB`.
fn memory_line(facts: &Facts, bar_width: usize) -> Line<'static> {
    let mut spans = field("memory", "", LABEL_WIDTH);
    spans.extend(memory_bar(facts, bar_width));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(text::gib(facts.resident_bytes()), BOLD));
    spans.push(Span::styled(
        format!(" of {} GiB", text::gib(facts.memory_bytes as i64)),
        DIM,
    ));
    Line::from(spans)
}

/// One `█` run per resident, sized by its share of the machine, then `░`s.
fn memory_bar(facts: &Facts, bar_width: usize) -> Vec<Span<'static>> {
    let total = facts.memory_bytes.max(1) as f64;
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (index, resident) in facts.residents.iter().enumerate() {
        let cells = ((resident.bytes.max(0) as f64 / total) * bar_width as f64).round() as usize;
        // Even a tiny resident gets a cell, so the legend never names a
        // segment that is not there.
        let cells = cells.max(1).min(bar_width - used);
        if cells == 0 {
            continue;
        }
        used += cells;
        spans.push(Span::styled(
            "█".repeat(cells),
            SEGMENT_STYLES[index % SEGMENT_STYLES.len()],
        ));
    }
    spans.push(Span::styled("░".repeat(bar_width - used), DIM));
    spans
}

/// `■ qwen3.5 6.1  ■ llava 4.7  · 49.8 free`.
fn legend_line(facts: &Facts) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(LABEL_WIDTH + 1))];
    for (index, resident) in facts.residents.iter().enumerate() {
        spans.push(Span::styled(
            "■ ",
            SEGMENT_STYLES[index % SEGMENT_STYLES.len()],
        ));
        spans.push(Span::styled(
            format!("{} {}  ", resident.name, text::gib(resident.bytes)),
            DIM,
        ));
    }
    spans.push(Span::styled(
        format!("· {} free", text::gib(facts.free_bytes())),
        DIM,
    ));
    Line::from(spans)
}

/// `disk    40.3 GB · ollama 27.8 · hf 12.4`.
fn disk_line(facts: &Facts) -> Line<'static> {
    let mut spans = field("disk", "", LABEL_WIDTH);
    spans.push(Span::styled(text::bytes(facts.disk_bytes()), BOLD));
    let stores: Vec<String> = facts
        .disk_by_store
        .iter()
        .filter(|(_, bytes)| *bytes > 0)
        .map(|(kind, bytes)| format!("{} {}", text::short_store(kind), text::bytes(*bytes)))
        .collect();
    if !stores.is_empty() {
        spans.push(Span::styled(format!(" · {}", stores.join(" · ")), DIM));
    }
    Line::from(spans)
}
