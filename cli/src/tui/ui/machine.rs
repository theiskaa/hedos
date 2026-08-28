//! The machine block under the shelf: what memory holds, what disk holds,
//! and the gateway beside it, or inside it when the layout is stacked.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{BAR_EMPTY, BAR_FILLED, BOLD, DIM, WARM, label, label_width};
use crate::tui::app::App;
use crate::tui::facts::Facts;
use crate::tui::text;

/// The labels of the rows the block always draws; the column is as wide
/// as the widest drawn, plus a gap, so the side-by-side block does not
/// widen for a row it never shows.
const MACHINE_LABELS: [&str; 2] = ["memory", "disk"];
/// The label of the gateway row, drawn only when the layout is stacked.
const GATEWAY_LABEL: &str = "gateway";
/// The steps the memory bar cycles through, one per resident: three
/// brightnesses of the plain foreground.
const SEGMENT_STYLES: [Style; 3] = [BOLD, Style::new(), DIM];
/// Cells the memory figure to the right of the bar needs: `  14.2 of 64 GiB`.
const FIGURE_WIDTH: u16 = 18;
const MIN_BAR_WIDTH: u16 = 10;

/// How many lines the machine block needs: memory and disk, plus the legend
/// when something is loaded, plus the gateway when the layout is `stacked`
/// and there is no block beside it to carry it.
pub(super) fn lines(facts: &Facts, stacked: bool) -> u16 {
    let base = if facts.residents.is_empty() { 2 } else { 3 };
    base + u16::from(stacked)
}

/// Draw the machine block into `machine` and the gateway block into
/// `gateway`; each is skipped when its rect has no room. When `stacked`,
/// the gateway's state is a line of the machine block instead.
pub(super) fn draw(frame: &mut Frame, machine: Rect, gateway: Rect, app: &App, stacked: bool) {
    if machine.height > 0 {
        draw_machine(frame, machine, &app.facts, stacked);
    }
    if gateway.height > 0 && gateway.width > 0 {
        draw_gateway(frame, gateway, &app.facts);
    }
}

fn draw_machine(frame: &mut Frame, area: Rect, facts: &Facts, stacked: bool) {
    let block = Block::bordered().title(" machine ").border_style(DIM);
    let inner = block.inner(area);
    let labels = label_column(stacked);
    let bar_width = inner
        .width
        .saturating_sub(labels as u16 + 1 + FIGURE_WIDTH)
        .max(MIN_BAR_WIDTH) as usize;
    let mut lines = if facts.residents.is_empty() {
        vec![idle_memory_line(facts, labels), disk_line(facts, labels)]
    } else {
        vec![
            memory_line(facts, bar_width, inner.width as usize, labels),
            legend_line(facts, inner.width as usize, labels),
            disk_line(facts, labels),
        ]
    };
    if stacked {
        lines.push(gateway_line(facts, labels));
    }
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_gateway(frame: &mut Frame, area: Rect, facts: &Facts) {
    let block = Block::bordered().title(" gateway ").border_style(DIM);
    let inner = block.inner(area);
    let state = match facts.gateway_port {
        Some(_) => Span::styled(format!(" {}", facts.gateway_state()), WARM),
        None => Span::styled(" off", DIM),
    };
    let lines = vec![
        Line::from(state),
        Line::from(Span::styled(format!(" {}", served_line(facts)), DIM)),
    ];
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The labels the block draws: the machine's, and the gateway's when
/// `stacked`, since its row joins the block.
fn labels(stacked: bool) -> Vec<&'static str> {
    let mut labels = MACHINE_LABELS.to_vec();
    if stacked {
        labels.push(GATEWAY_LABEL);
    }
    labels
}

/// The width of the label column over the [`labels`] drawn.
fn label_column(stacked: bool) -> usize {
    label_width(&labels(stacked), 1)
}

/// `memory  nothing loaded · 64 GiB free`.
fn idle_memory_line(facts: &Facts, labels: usize) -> Line<'static> {
    Line::from(vec![
        label("memory", labels),
        Span::styled("nothing loaded", DIM),
        Span::raw(" · "),
        Span::styled(text::gib(facts.free_bytes()), BOLD),
        Span::styled(" GiB free", DIM),
    ])
}

/// `gateway  on :11434 · 3 req/min`, the state warm and the figures dim,
/// or a dim `off`: the gateway block's first line, folded into the machine
/// block when there is no room beside it.
fn gateway_line(facts: &Facts, labels: usize) -> Line<'static> {
    let mut spans = vec![label(GATEWAY_LABEL, labels)];
    match facts.gateway_port {
        Some(port) => {
            spans.push(Span::styled("on", WARM));
            spans.push(Span::styled(
                format!(" :{port} · {} req/min", facts.activity.requests_last_minute),
                DIM,
            ));
        }
        None => spans.push(Span::styled("off", DIM)),
    }
    Line::from(spans)
}

/// `last request 21d ago · 87 all time`, or a quiet note when the log is empty.
fn served_line(facts: &Facts) -> String {
    let activity = &facts.activity;
    if activity.total_requests == 0 {
        return "nothing served yet".to_owned();
    }
    format!(
        "last request {} ago · {} all time",
        text::duration((facts.collected_at_millis - activity.last_request_millis) / 1000),
        text::count(activity.total_requests as usize, "request")
    )
}

/// `memory  ████░░░░  14.2 of 64 GiB`, held to `width` cells: the bar has
/// a floor, so a block too narrow for it and the figure clips the figure.
fn memory_line(facts: &Facts, bar_width: usize, width: usize, labels: usize) -> Line<'static> {
    let mut spans = vec![label("memory", labels)];
    spans.extend(memory_bar(facts, bar_width));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(text::gib(facts.resident_bytes()), BOLD));
    spans.push(Span::styled(
        format!(" of {} GiB", text::gib(facts.memory_bytes as i64)),
        DIM,
    ));
    clipped(spans, width)
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
            BAR_FILLED.repeat(cells),
            SEGMENT_STYLES[index % SEGMENT_STYLES.len()],
        ));
    }
    spans.push(Span::styled(BAR_EMPTY.repeat(bar_width - used), DIM));
    spans
}

/// `■ qwen3.5 6.1  ■ llava 4.7  · 49.8 free`, held to `width` cells: the
/// free figure goes first, then the names are clipped.
fn legend_line(facts: &Facts, width: usize, labels: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(labels + 1))];
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
    let free = Span::styled(format!("· {} free", text::gib(facts.free_bytes())), DIM);
    let used: usize = spans.iter().map(Span::width).sum();
    if used + free.width() <= width {
        spans.push(free);
        return Line::from(spans);
    }
    clipped(spans, width)
}

/// `spans` cut to `width` cells, the one it lands in clipped with `…`.
fn clipped(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut kept = Vec::new();
    let mut used = 0;
    for span in spans {
        if used + span.width() <= width {
            used += span.width();
            kept.push(span);
            continue;
        }
        let cut = text::clip(span.content.trim_end(), width - used);
        if !cut.is_empty() {
            kept.push(Span::styled(cut, span.style));
        }
        break;
    }
    Line::from(kept)
}

/// `disk    40.3 GB · ollama 27.8 · hf 12.4`.
fn disk_line(facts: &Facts, labels: usize) -> Line<'static> {
    let mut spans = vec![label("disk", labels)];
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

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

    use crate::support::residency::Holder;
    use crate::tui::testing::{facts_with_memory, leading_label, resident_with_bytes, text};

    #[test]
    fn every_label_is_listed() {
        let facts = Facts {
            residents: vec![resident_with_bytes("m", Holder::Local, 4 << 30)],
            disk_by_store: vec![("ollama".to_owned(), 1 << 30)],
            ..facts_with_memory(64)
        };
        let listed = super::labels(true);
        let labels = label_column(true);
        let mut seen = std::collections::HashSet::new();
        for line in [
            idle_memory_line(&facts, labels),
            memory_line(&facts, 10, 80, labels),
            disk_line(&facts, labels),
            gateway_line(&facts, labels),
        ] {
            let label = leading_label(&line, labels);
            assert!(listed.contains(&label.as_str()), "{label} is not listed");
            seen.insert(label);
        }
        assert_eq!(seen.len(), listed.len());
        assert_eq!(leading_label(&legend_line(&facts, 80, labels), labels), "");
        assert!(text(&idle_memory_line(&facts, labels)).ends_with("nothing loaded · 60 GiB free"));
    }

    #[test]
    fn the_label_column_widens_for_the_gateway_only_when_stacked() {
        assert_eq!(label_column(false), "memory".len() + 1);
        assert_eq!(label_column(true), "gateway".len() + 1);
        let facts = Facts::default();
        let disk = text::bytes(0);
        assert_eq!(
            text(&disk_line(&facts, label_column(false))),
            format!(" disk   {disk}")
        );
        assert_eq!(
            text(&disk_line(&facts, label_column(true))),
            format!(" disk    {disk}")
        );
    }

    #[test]
    fn the_gateway_line_joins_the_block_only_when_stacked() {
        let off = Facts::default();
        assert_eq!(lines(&off, false), 2);
        assert_eq!(lines(&off, true), 3);
        let labels = label_column(true);
        let line = gateway_line(&off, labels);
        assert!(text(&line).ends_with("gateway off"));
        assert_eq!(line.spans[1].style, DIM);
        let on = Facts {
            gateway_port: Some(11434),
            ..Facts::default()
        };
        let line = gateway_line(&on, labels);
        assert!(text(&line).ends_with("gateway on :11434 · 0 req/min"));
        assert_eq!(line.spans[1].style, WARM);
        assert_eq!(line.spans[2].style, DIM);
    }

    #[test]
    fn the_memory_line_clips_to_the_block_under_the_bar_floor() {
        let facts = Facts {
            residents: vec![resident_with_bytes("m", Holder::Local, 4 << 30)],
            ..facts_with_memory(64)
        };
        let labels = label_column(false);
        let full = memory_line(&facts, MIN_BAR_WIDTH as usize, 80, labels);
        assert!(text(&full).ends_with("4 of 64 GiB"), "{:?}", text(&full));
        let narrow = full.width() - 3;
        let cut = memory_line(&facts, MIN_BAR_WIDTH as usize, narrow, labels);
        assert!(cut.width() <= narrow, "{:?}", text(&cut));
        assert!(text(&cut).ends_with('…'), "{:?}", text(&cut));
        assert_eq!(
            text(&cut)
                .chars()
                .filter(|c| *c == '█' || *c == '░')
                .count(),
            MIN_BAR_WIDTH as usize
        );
    }

    #[test]
    fn the_legend_never_runs_past_the_block() {
        let resident = |name: &str| resident_with_bytes(name, Holder::Local, 4 << 30);
        let facts = Facts {
            residents: vec![
                resident("qwen2.5-coder"),
                resident("llava-phi3-mini"),
                resident("deepseek-r1-distill"),
            ],
            ..facts_with_memory(64)
        };
        let labels = label_column(false);
        let full = legend_line(&facts, 120, labels);
        let free = format!("· {} free", text::gib(facts.free_bytes()));
        assert!(text(&full).ends_with(&free));
        assert_eq!(free, "· 52 free");
        for width in [78, 60, 40, 20] {
            let line = legend_line(&facts, width, labels);
            assert!(
                line.width() <= width,
                "{:?} is {} cells at {width}",
                text(&line),
                line.width()
            );
        }
        // One cell short of the free figure: the names stay whole, it goes.
        let no_free = text(&legend_line(
            &facts,
            full.width() - free.width() + 1,
            labels,
        ));
        assert!(!no_free.contains("free") && no_free.contains("deepseek-r1-distill 4"));
        let cut = text(&legend_line(&facts, 40, labels));
        assert!(cut.ends_with('…'), "{cut:?}");
        assert!(!cut.contains("free"));
    }
}
