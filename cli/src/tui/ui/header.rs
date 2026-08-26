//! The header: one line of numbers when space is short, the koala beside a
//! machine panel when there is room.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BOLD, DIM, field};
use crate::support::banner::{KOALA, KOALA_WIDTH};
use crate::tui::app::App;
use crate::tui::facts::Facts;
use crate::tui::layout::TALL_HEADER_ROWS;
use crate::tui::text;

/// Width of the labels in the machine panel.
const LABEL_WIDTH: usize = 8;
/// The three brightness steps the memory bar cycles through, one per resident.
const SEGMENT_STYLES: [Style; 3] = [BOLD, Style::new(), DIM];
/// Cells the memory figure to the right of the bar needs: `  14.2 of 64 GiB`.
const FIGURE_WIDTH: u16 = 18;
/// The bar never shrinks below this, whatever the panel width.
const MIN_BAR_WIDTH: u16 = 10;

/// Draw the header into `area`, tall or one-line by its height.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if area.height >= TALL_HEADER_ROWS {
        draw_tall(frame, area, app);
    } else {
        frame.render_widget(Paragraph::new(summary_line(app)), area);
    }
}

fn summary_line(app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(" hedos", BOLD),
        Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} · {} warm · {} on disk · gateway {}",
                text::count(app.records.len(), "model"),
                app.warm_count(),
                text::bytes(app.facts.disk_bytes()),
                gateway_state(&app.facts, false),
            ),
            DIM,
        ),
    ])
}

/// `on :11434 · 3 req/min` or `off`, with the hint to start one when `hint`
/// is set.
fn gateway_state(facts: &Facts, hint: bool) -> String {
    match (facts.gateway_port, hint) {
        (Some(port), _) => format!(
            "on :{port} · {} req/min",
            facts.activity.requests_last_minute
        ),
        (None, true) => "off · hedos serve to start".to_owned(),
        (None, false) => "off".to_owned(),
    }
}

fn draw_tall(frame: &mut Frame, area: Rect, app: &App) {
    let [koala, panel] =
        Layout::horizontal([Constraint::Length(KOALA_WIDTH + 5), Constraint::Min(0)]).areas(area);
    let koala_lines: Vec<Line> = KOALA
        .iter()
        .map(|row| Line::from(Span::styled(format!("  {row}"), BOLD)))
        .collect();
    frame.render_widget(Paragraph::new(koala_lines), koala);

    let facts = &app.facts;
    let row = |label, value: String| Line::from(field(label, value, LABEL_WIDTH));
    let bar_width = panel
        .width
        .saturating_sub(LABEL_WIDTH as u16 + 1 + FIGURE_WIDTH)
        .max(MIN_BAR_WIDTH) as usize;
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(" hedos", BOLD),
            Span::styled(format!(" v{}", env!("CARGO_PKG_VERSION")), DIM),
        ]),
        Line::from(Span::styled(" ἕδος · a seat, an abode, a foundation", DIM)),
        Line::default(),
        memory_line(facts, bar_width),
        legend_line(facts),
        disk_line(facts),
        row("gateway", gateway_state(facts, true)),
        row("shelf", shelf_line(app)),
    ];
    lines.truncate(area.height as usize);
    frame.render_widget(Paragraph::new(lines), panel);
}

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
        let cells = cells.min(bar_width - used);
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

fn disk_line(facts: &Facts) -> Line<'static> {
    let mut spans = field("disk", "", LABEL_WIDTH);
    spans.push(Span::styled(text::bytes(facts.disk_bytes()), BOLD));
    let stores: Vec<String> = facts
        .disk_by_store
        .iter()
        .map(|(kind, bytes)| format!("{kind} {}", text::bytes(*bytes)))
        .collect();
    if !stores.is_empty() {
        spans.push(Span::styled(format!(" · {}", stores.join(" · ")), DIM));
    }
    Line::from(spans)
}

fn shelf_line(app: &App) -> String {
    let mut parts = vec![text::count(app.records.len(), "model")];
    parts.push(format!("{} warm", app.warm_count()));
    let too_big = app.too_big_count();
    if too_big > 0 {
        parts.push(format!("{too_big} too big for this machine"));
    }
    parts.join(" · ")
}
