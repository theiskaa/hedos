//! The machine card under the shelf: what memory holds, what disk holds;
//! and the gateway card beside it, or a line inside it when the layout is
//! stacked.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::card::Card;
use super::detail::{Look, sparkline};
use super::header::pulse;
use super::{
    ACCENT_MID, BAR_EMPTY, BAR_FILLED, BOLD, DIM, SEGMENT_3, SOFT, TRACK, card, label, label_width,
};
use crate::support::clock;
use crate::tui::app::App;
use crate::tui::facts::{Facts, HOURS};
use crate::tui::motion::Motion;
use crate::tui::palette::INK_COLOR;
use crate::tui::text;

/// The labels of the rows the card always draws; the column is as wide
/// as the widest drawn, plus a gap, so the side-by-side card does not
/// widen for a row it never shows.
const MACHINE_LABELS: [&str; 2] = ["memory", "disk"];
/// The label of the gateway row, drawn only when the layout is stacked.
const GATEWAY_LABEL: &str = "gateway";
/// The greys the memory bar cycles through, one per resident, so the legend
/// tells the segments apart; they mean nothing beyond that.
const SEGMENT_STYLES: [Style; 3] = [
    Style::new().fg(INK_COLOR),
    Style::new().fg(ACCENT_MID),
    Style::new().fg(SEGMENT_3),
];
/// Cells the memory figure to the right of the bar needs: `  14.2 of 64 GiB`.
const FIGURE_WIDTH: u16 = 18;
const MIN_BAR_WIDTH: u16 = 10;
/// How long a model's segment takes to grow in or shrink away.
pub(crate) const SEGMENT_MS: u64 = 300;
/// The words beside the gateway's sparkline.
const SPARK_AXIS: &str = "  requests · 24h";

/// How many lines the machine card needs: memory, its legend, and disk,
/// plus the gateway when the layout is `stacked` and there is no card
/// beside it to carry it.
pub(super) fn lines(stacked: bool) -> u16 {
    3 + u16::from(stacked)
}

/// Draw the machine card into `machine` and the gateway card into
/// `gateway`; each is skipped when its rect has no room. When `stacked`,
/// the gateway's state is a line of the machine card instead.
pub(super) fn draw(frame: &mut Frame, machine: Rect, gateway: Rect, app: &App, stacked: bool) {
    if machine.height > 0 {
        draw_machine(frame, machine, app, stacked);
    }
    if gateway.height > 0 && gateway.width > 0 {
        draw_gateway(frame, gateway, app);
    }
}

/// Each resident's bytes as the bar shows them now: easing in after a
/// warm, easing out after an unload.
fn shown(app: &App) -> Vec<(String, i64)> {
    let items: Vec<(String, String, f64)> = app
        .facts
        .residents
        .iter()
        .map(|resident| {
            (
                resident.id.clone(),
                resident.name.clone(),
                resident.bytes.max(0) as f64,
            )
        })
        .collect();
    app.resident_bars
        .values(&items, SEGMENT_MS, &app.motion)
        .into_iter()
        .map(|(name, bytes)| (name, bytes.round() as i64))
        .collect()
}

fn draw_machine(frame: &mut Frame, area: Rect, app: &App, stacked: bool) {
    let facts = &app.facts;
    card("machine")
        .right(vec![Span::styled(
            format!("{} GiB", text::gib(facts.memory_bytes as i64)),
            DIM,
        )])
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let labels = label_column(stacked);
    let bar_width = inner
        .width
        .saturating_sub(labels as u16 + 1 + FIGURE_WIDTH)
        .max(MIN_BAR_WIDTH) as usize;
    let shown = shown(app);
    let mut lines = vec![
        memory_line(facts, &shown, bar_width, inner.width as usize, labels),
        legend_line(facts, &shown, inner.width as usize, labels),
        disk_line(facts, labels),
    ];
    if stacked {
        lines.push(gateway_line(facts, labels, &app.motion));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_gateway(frame: &mut Frame, area: Rect, app: &App) {
    let facts = &app.facts;
    card("gateway")
        .right(vec![Span::styled("openai · ollama · anthropic", DIM)])
        .render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let width = inner.width as usize;
    let mut state = vec![Span::raw(" ")];
    state.extend(gateway_state(facts, &app.motion));
    let mut lines = vec![
        Line::from(state),
        Line::from(Span::styled(
            format!(
                " {}",
                text::clip(&served_line(facts), width.saturating_sub(1))
            ),
            DIM,
        )),
    ];
    if facts.activity.hourly.iter().any(|count| *count > 0) && width > HOURS + 1 {
        let look = Look {
            motion: &app.motion,
            selected_at: 0,
            busy: false,
            spin_frame: 0,
        };
        let mut spark = vec![Span::raw(" ")];
        spark.extend(sparkline(&facts.activity.hourly, &look));
        if HOURS + 1 + SPARK_AXIS.len() <= width {
            spark.push(Span::styled(SPARK_AXIS, DIM));
        }
        lines.push(Line::from(spark));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `● on · 127.0.0.1:11434 · 3 req/min` with the dot pulsing, or `○ off`
/// and the key that starts it.
fn gateway_state(facts: &Facts, motion: &Motion) -> Vec<Span<'static>> {
    match facts.gateway_port {
        Some(port) => vec![
            Span::styled("●", Style::new().fg(pulse(motion))),
            Span::styled(" on", BOLD),
            Span::styled(
                format!(
                    " · 127.0.0.1:{port} · {} req/min",
                    facts.activity.requests_last_minute
                ),
                SOFT,
            ),
        ],
        None => vec![
            Span::styled("○ off", DIM),
            Span::raw("  "),
            Span::styled("S", BOLD),
            Span::styled(" serve", DIM),
        ],
    }
}

/// The labels the card draws: the machine's, and the gateway's when
/// `stacked`, since its row joins the card.
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

/// `gateway  ● on :11434 · 3 req/min`, or `○ off`: the gateway card's first
/// line, folded into the machine card when there is no room beside it.
fn gateway_line(facts: &Facts, labels: usize, motion: &Motion) -> Line<'static> {
    let mut spans = vec![label(GATEWAY_LABEL, labels)];
    match facts.gateway_port {
        Some(port) => {
            spans.push(Span::styled("●", Style::new().fg(pulse(motion))));
            spans.push(Span::styled(" on", BOLD));
            spans.push(Span::styled(
                format!(" :{port} · {} req/min", facts.activity.requests_last_minute),
                SOFT,
            ));
        }
        None => spans.push(Span::styled("○ off", DIM)),
    }
    Line::from(spans)
}

/// `last request 21d ago · 11,376 requests all time`, or a quiet note when
/// the log is empty.
fn served_line(facts: &Facts) -> String {
    let activity = &facts.activity;
    if activity.total_requests == 0 {
        return "nothing served yet".to_owned();
    }
    let total = activity.total_requests;
    format!(
        "last request {} ago · {} {} all time",
        clock::duration((facts.collected_at_millis - activity.last_request_millis) / 1000),
        grouped(total),
        if total == 1 { "request" } else { "requests" }
    )
}

/// `11376` as `11,376`.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `memory  ━━━━━━━━━━  14.2 of 64 GiB`, held to `width` cells: the bar has
/// a floor, so a card too narrow for it and the figure clips the figure.
fn memory_line(
    facts: &Facts,
    shown: &[(String, i64)],
    bar_width: usize,
    width: usize,
    labels: usize,
) -> Line<'static> {
    let mut spans = vec![label("memory", labels)];
    spans.extend(memory_bar(facts, shown, bar_width));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(text::gib(facts.resident_bytes()), BOLD));
    spans.push(Span::styled(
        format!(" of {} GiB", text::gib(facts.memory_bytes as i64)),
        SOFT,
    ));
    clipped(spans, width)
}

/// One run per resident, sized by its share of the machine as it shows
/// now, then the track.
fn memory_bar(facts: &Facts, shown: &[(String, i64)], bar_width: usize) -> Vec<Span<'static>> {
    let total = facts.memory_bytes.max(1) as f64;
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (index, (_, bytes)) in shown.iter().enumerate() {
        if *bytes <= 0 {
            continue;
        }
        let cells = ((*bytes as f64 / total) * bar_width as f64).round() as usize;
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
    spans.push(Span::styled(BAR_EMPTY.repeat(bar_width - used), TRACK));
    spans
}

/// `■ qwen3.5 6.1  ■ llava 4.7  · 49.8 free`, or `nothing loaded · 64 free`,
/// held to `width` cells: the free figure goes first, then the names are
/// clipped.
fn legend_line(
    facts: &Facts,
    shown: &[(String, i64)],
    width: usize,
    labels: usize,
) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(labels + 1))];
    let free = text::gib(facts.free_bytes());
    if shown.iter().all(|(_, bytes)| *bytes <= 0) {
        spans.push(Span::styled(format!("nothing loaded · {free} free"), DIM));
        return clipped(spans, width);
    }
    for (index, (name, bytes)) in shown.iter().enumerate() {
        if *bytes <= 0 {
            continue;
        }
        spans.push(Span::styled(
            "■ ",
            SEGMENT_STYLES[index % SEGMENT_STYLES.len()],
        ));
        spans.push(Span::styled(
            format!("{name} {}  ", text::gib(*bytes)),
            SOFT,
        ));
    }
    let free = Span::styled(format!("· {free} free"), DIM);
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

/// `disk    40.3 GB · ollama 27.8 · hf 12.4`, or `counting` until the
/// first count has finished.
fn disk_line(facts: &Facts, labels: usize) -> Line<'static> {
    let mut spans = vec![label("disk", labels)];
    let Some(stores) = &facts.disk_by_store else {
        spans.push(Span::styled("counting", DIM));
        return Line::from(spans);
    };
    let total: i64 = stores.iter().map(|(_, bytes)| bytes).sum();
    spans.push(Span::styled(text::bytes(total), BOLD));
    let stores: Vec<String> = stores
        .iter()
        .filter(|(_, bytes)| *bytes > 0)
        .map(|(kind, bytes)| format!("{} {}", text::short_store(kind), text::bytes(*bytes)))
        .collect();
    if !stores.is_empty() {
        spans.push(Span::styled(format!(" · {}", stores.join(" · ")), SOFT));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

    use crate::support::residency::Holder;
    use crate::tui::palette::OLIVE;
    use crate::tui::testing::{facts_with_memory, leading_label, resident_with_bytes, text};

    /// What a settled bar shows: every resident at its size.
    fn settled(facts: &Facts) -> Vec<(String, i64)> {
        facts
            .residents
            .iter()
            .map(|resident| (resident.name.clone(), resident.bytes))
            .collect()
    }

    #[test]
    fn every_label_is_listed() {
        let facts = Facts {
            residents: vec![resident_with_bytes("m", Holder::Local, 4 << 30)],
            disk_by_store: Some(vec![("ollama".to_owned(), 1 << 30)]),
            ..facts_with_memory(64)
        };
        let listed = super::labels(true);
        let labels = label_column(true);
        let mut seen = std::collections::HashSet::new();
        for line in [
            memory_line(&facts, &settled(&facts), 10, 80, labels),
            disk_line(&facts, labels),
            gateway_line(&facts, labels, &Motion::settled()),
        ] {
            let label = leading_label(&line, labels);
            assert!(listed.contains(&label.as_str()), "{label} is not listed");
            seen.insert(label);
        }
        assert_eq!(seen.len(), listed.len());
        assert_eq!(
            leading_label(&legend_line(&facts, &settled(&facts), 80, labels), labels),
            ""
        );
        let idle = facts_with_memory(64);
        assert!(text(&legend_line(&idle, &[], 80, labels)).ends_with("nothing loaded · 64 free"));
    }

    #[test]
    fn the_label_column_widens_for_the_gateway_only_when_stacked() {
        assert_eq!(label_column(false), "memory".len() + 1);
        assert_eq!(label_column(true), "gateway".len() + 1);
        let facts = Facts {
            disk_by_store: Some(Vec::new()),
            ..Facts::default()
        };
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
    fn the_disk_line_says_it_is_counting_until_the_first_count() {
        assert_eq!(
            text(&disk_line(&Facts::default(), label_column(false))),
            " disk   counting"
        );
    }

    #[test]
    fn the_gateway_line_joins_the_card_only_when_stacked() {
        assert_eq!(lines(false), 3);
        assert_eq!(lines(true), 4);
        let labels = label_column(true);
        let off = gateway_line(&Facts::default(), labels, &Motion::settled());
        assert!(text(&off).ends_with("gateway ○ off"), "{:?}", text(&off));
        assert_eq!(off.spans[1].style, DIM);
        let on = Facts {
            gateway_port: Some(11434),
            ..Facts::default()
        };
        let line = gateway_line(&on, labels, &Motion::settled());
        assert!(text(&line).ends_with("gateway ● on :11434 · 0 req/min"));
        assert_eq!(line.spans[1].style.fg, Some(OLIVE));
        assert_eq!(line.spans[3].style, SOFT);
    }

    #[test]
    fn the_served_line_groups_its_thousands() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(11376), "11,376");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn the_memory_line_clips_to_the_card_under_the_bar_floor() {
        let facts = Facts {
            residents: vec![resident_with_bytes("m", Holder::Local, 4 << 30)],
            ..facts_with_memory(64)
        };
        let labels = label_column(false);
        let shown = settled(&facts);
        let full = memory_line(&facts, &shown, MIN_BAR_WIDTH as usize, 80, labels);
        assert!(text(&full).ends_with("4 of 64 GiB"), "{:?}", text(&full));
        let narrow = full.width() - 3;
        let cut = memory_line(&facts, &shown, MIN_BAR_WIDTH as usize, narrow, labels);
        assert!(cut.width() <= narrow, "{:?}", text(&cut));
        assert!(text(&cut).ends_with('…'), "{:?}", text(&cut));
        assert_eq!(
            text(&cut).chars().filter(|c| *c == '━').count(),
            MIN_BAR_WIDTH as usize
        );
    }

    #[test]
    fn a_segment_shrinking_away_still_reads_in_the_bar() {
        let facts = facts_with_memory(64);
        let shown = vec![("m".to_owned(), 8i64 << 30)];
        let bar: String = memory_bar(&facts, &shown, 16)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(bar.chars().count(), 16);
        let line = text(&legend_line(&facts, &shown, 80, label_column(false)));
        assert!(line.contains("■ m 8"), "{line:?}");
    }

    #[test]
    fn the_legend_never_runs_past_the_card() {
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
        let shown = settled(&facts);
        let full = legend_line(&facts, &shown, 120, labels);
        let free = format!("· {} free", text::gib(facts.free_bytes()));
        assert!(text(&full).ends_with(&free));
        assert_eq!(free, "· 52 free");
        for width in [78, 60, 40, 20] {
            let line = legend_line(&facts, &shown, width, labels);
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
            &shown,
            full.width() - free.width() + 1,
            labels,
        ));
        assert!(!no_free.contains("free") && no_free.contains("deepseek-r1-distill 4"));
        let cut = text(&legend_line(&facts, &shown, 40, labels));
        assert!(cut.ends_with('…'), "{cut:?}");
        assert!(!cut.contains("free"));
    }
}
