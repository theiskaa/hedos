//! The model card: the selected record's name and facts, its capabilities,
//! how it fits beside what is already loaded, its residency, what the
//! gateway has served of it, and where its weights are.

use kernel::records::{Capability, ModelRecord, ModelState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use unicode_width::UnicodeWidthStr;

use super::card::Card;
use super::{
    ACCENT, BOLD, DIM, INK, SOFT, card, field_line, label, label_width, section, spinner,
    styled_field, value_width,
};
use crate::support::clock;
use crate::support::residency::Holder;
use crate::support::shelf_table::runtime_label;
use crate::support::table::DASH;
use crate::support::text::printable;
use crate::tui::app::App;
use crate::tui::facts::{Facts, HOURS, ModelActivity};
use crate::tui::layout::STACKED_DETAIL_ROWS;
use crate::tui::motion::Motion;
use crate::tui::palette::{
    ACCENT_DIM, BRIGHT, FAINT_COLOR, GHOST, INK_COLOR, LEVELS, LINE, LINE_STRONG, OLIVE, OLIVE_DIM,
    RAISED, SOFT_COLOR, mix,
};
use crate::tui::text;

/// The labels the pane uses; the column is as wide as the widest, plus a gap.
const LABELS: [&str; 14] = [
    "size",
    "fit",
    "residency",
    "last used",
    "last 24h",
    "latency",
    "path",
    "id",
    "runtime id",
    "store id",
    "alias",
    "modality",
    "execution",
    "state",
];

/// What the pane says the gateway has seen of a model that never came
/// through it.
const NO_GATEWAY_REQUESTS: &str = "no requests through the gateway";
/// How fast a name arrives, and how long its facts take to come up.
const REVEAL_PER_MS: u64 = 4;
const SUB_FADE_MS: u64 = 120;
/// How long the model's share of the fit gauge takes to grow.
const GAUGE_GROW_MS: u64 = 220;
/// The widest the fit gauge gets, and the room its suffix takes.
const GAUGE_MAX: usize = 40;
const GAUGE_SUFFIX: usize = 10;
/// How the sparkline's bars rise: each hour a step behind the last.
const SPARK_STEP_MS: u64 = 3;
const SPARK_RISE_MS: u64 = 150;
/// The words beside a non-expanded sparkline.
const SPARK_AXIS: &str = "  24h ago … now";

/// What the card needs beyond the record and the facts: the clock, when the
/// selection last changed, and whether a task is under way on the model.
pub(super) struct Look<'a> {
    pub motion: &'a Motion,
    pub selected_at: u64,
    pub busy: bool,
    pub spin_frame: u64,
}

impl Look<'_> {
    /// The card as it reads once every movement has finished.
    #[cfg(test)]
    pub(super) fn settled(motion: &Motion) -> Look<'_> {
        Look {
            motion,
            selected_at: 0,
            busy: false,
            spin_frame: 0,
        }
    }

    /// A breath between 0 and 1 on a period of about three seconds, a fixed
    /// rest value when nothing moves.
    fn breath(&self, rate: f32, phase: f32) -> f32 {
        self.motion
            .age(0)
            .map_or(0.6, |age| 0.5 + 0.5 * (age * rate + phase).sin())
    }
}

/// The width of the label column.
fn label_column() -> usize {
    label_width(&LABELS, 1)
}

/// Draw the model card into `area` for the selected model.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(record) = app.selected_record() else {
        card("model").render(area, frame.buffer_mut());
        return;
    };
    let look = Look {
        motion: &app.motion,
        selected_at: app.selected_at(),
        busy: app.tasks.running_on(&record.id),
        spin_frame: app.spin_frame(),
    };
    let compact = area.height <= STACKED_DETAIL_ROWS;
    let card = if compact {
        Card::new(vec![Span::styled(title(record).trim().to_owned(), BOLD)]).right(vec![
            Span::styled(
                record.serving_size().map_or(DASH.to_owned(), text::bytes),
                SOFT,
            ),
        ])
    } else {
        // Expanded, the card is a mode of its own: its title and border
        // brighten.
        let card = Card::new(vec![Span::styled(
            "model",
            if app.expanded { ACCENT } else { BOLD },
        )])
        .right(vec![Span::styled(
            format!("{} of {}", app.selected() + 1, app.order.len()),
            DIM,
        )]);
        if app.expanded { card.border(DIM) } else { card }
    };
    card.render(area, frame.buffer_mut());
    let inner = Card::text_inner(area);
    let width = inner.width as usize;
    let lines = if compact {
        compact_lines(record, &app.facts, width)
    } else {
        let mut lines = full_lines(record, &app.facts, app.expanded, width, &look);
        pin_path(&mut lines, record, inner.height as usize);
        lines
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The path row moved to the card's last row when there is room under the
/// rest, so it sits at the foot of the card like a footnote.
fn pin_path(lines: &mut Vec<Line<'static>>, record: &ModelRecord, height: usize) {
    if record.primary_weight_path.is_none() || lines.len() >= height {
        return;
    }
    if let Some(path) = lines.pop() {
        lines.resize(height - 1, Line::default());
        lines.push(path);
    }
}

/// What the pane appends to a path whose file is no longer there.
const GONE_SUFFIX: &str = " · gone";

/// A `label   value` row, the value clipped to `value_width`.
fn row(label: &str, value: &str, value_width: usize, style: Style) -> Line<'static> {
    Line::from(styled_field(
        label,
        text::clip(value, value_width),
        label_column(),
        style,
    ))
}

/// The stacked pane's four rows: what the shelf row does not already show,
/// the size standing in for a path the record does not have.
fn compact_lines(record: &ModelRecord, facts: &Facts, width: usize) -> Vec<Line<'static>> {
    let value_width = value_width(width, label_column());
    vec![
        row("fit", &fit_line(record, facts), value_width, Style::new()),
        residency_line(record, facts, value_width, None),
        activity_line(
            facts.activity.for_record(record),
            facts.collected_at_millis,
            value_width,
        ),
        path_line(record, value_width).unwrap_or_else(|| size_line(record, value_width)),
    ]
}

/// The card's rows at `width` cells: the name and its facts, the
/// capabilities, MEMORY (fit, the gauge, residency), GATEWAY (traffic and
/// the day's sparkline), and when `expanded` the record's identifiers; the
/// path last. A value that would run past the edge is clipped, a path
/// elided in the middle.
fn full_lines(
    record: &ModelRecord,
    facts: &Facts,
    expanded: bool,
    width: usize,
    look: &Look,
) -> Vec<Line<'static>> {
    let value_width = value_width(width, label_column());
    let row = |label, value: String| row(label, &value, value_width, Style::new());
    let name = printable(record.display_name()).into_owned();
    let mut name_spans = vec![Span::raw(" ")];
    name_spans.extend(
        look.motion
            .reveal(
                &text::clip(&name, width.saturating_sub(1)),
                look.selected_at,
                REVEAL_PER_MS,
            )
            .into_iter()
            .map(|(piece, ramp)| Span::styled(piece, if ramp { DIM } else { BOLD })),
    );
    let sub_style = Style::new().fg(mix(
        FAINT_COLOR,
        SOFT_COLOR,
        look.motion.eased(look.selected_at, SUB_FADE_MS),
    ));
    let mut lines = vec![
        Line::default(),
        Line::from(name_spans),
        Line::from(Span::styled(
            format!(
                " {}",
                text::clip(&sub_line(record), width.saturating_sub(1))
            ),
            sub_style,
        )),
        Line::default(),
        chips_line(record, width),
        Line::default(),
        section("MEMORY", width),
        fit_row(record, facts, value_width),
    ];
    lines.extend(gauge_row(record, facts, value_width, look));
    lines.push(residency_line(record, facts, value_width, Some(look)));
    lines.push(Line::default());
    lines.push(section("GATEWAY", width));
    lines.extend(activity_lines(
        facts.activity.for_record(record),
        facts.collected_at_millis,
        expanded,
        value_width,
        look,
    ));
    if expanded {
        lines.push(Line::default());
        lines.push(section("RECORD", width));
        lines.push(row("id", record.id.clone()));
        lines.push(row("runtime id", runtime_label(record).to_owned()));
        lines.push(row("store id", record.source.kind.as_str().to_owned()));
        if let Some(alias) = &record.alias {
            lines.push(row("alias", alias.clone()));
        }
        lines.push(row("modality", record.modality.as_str().to_owned()));
        lines.push(row("execution", record.execution.as_str().to_owned()));
        lines.push(row("state", record.state.as_str().to_owned()));
    }
    if let Some(path) = path_line(record, value_width) {
        lines.push(Line::default());
        lines.push(path);
    }
    lines
}

/// `mlx-lm · hf · 289 MB · ctx 32k`, plus what the store holds on disk when
/// that differs from what serving loads: `… · 34 GB on disk`.
fn sub_line(record: &ModelRecord) -> String {
    let runtime = match text::short_runtime(runtime_label(record)) {
        DASH => "no runtime",
        runtime => runtime,
    };
    let mut parts = vec![
        runtime.to_owned(),
        text::short_store(record.source.kind.as_str()).to_owned(),
    ];
    if record.state == ModelState::Missing {
        parts.push("weights gone".to_owned());
    } else if let Some(bytes) = record.serving_size() {
        parts.push(text::bytes(bytes));
    }
    if let Some(context) = record.context_length {
        parts.push(format!("ctx {}", text::tokens(context)));
    }
    if let Some(disk) = record.size_on_disk()
        && record.serving_size() != Some(disk)
        && record.state != ModelState::Missing
    {
        parts.push(format!("{} on disk", text::bytes(disk)));
    }
    parts.join(" · ")
}

/// The capabilities as chips, ` chat ` on a raised ground with a space
/// between; chips that would run past `width` are left off.
fn chips_line(record: &ModelRecord, width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for capability in record.capabilities.iter().map(Capability::as_str) {
        let chip = format!(" {capability} ");
        let need = chip.width() + usize::from(used > 1);
        if used + need > width {
            break;
        }
        if used > 1 {
            spans.push(Span::raw(" "));
        }
        used += need;
        spans.push(Span::styled(chip, SOFT.bg(RAISED)));
    }
    Line::from(spans)
}

/// `fit   fits · needs 1 of 64 GiB · …`, the verdict bold.
fn fit_row(record: &ModelRecord, facts: &Facts, value_width: usize) -> Line<'static> {
    let fit = text::clip(&fit_line(record, facts), value_width);
    let mut spans = vec![label("fit", label_column())];
    match fit.split_once(" · ") {
        Some((verdict, rest)) if record.state != ModelState::Missing => {
            spans.push(Span::styled(verdict.to_owned(), BOLD));
            spans.push(Span::styled(format!(" · {rest}"), SOFT));
        }
        _ => spans.push(Span::styled(fit, SOFT)),
    }
    Line::from(spans)
}

/// The fit as a gauge of the machine's memory: what the rest of the loaded
/// models hold, dim; what this one needs, in ink when it is held and
/// breathing when it is only what warming it would take; the rest a track.
/// Nothing for a model whose size is not known.
fn gauge_row(
    record: &ModelRecord,
    facts: &Facts,
    value_width: usize,
    look: &Look,
) -> Option<Line<'static>> {
    let needed = record.serving_size()?;
    let cells = value_width.saturating_sub(GAUGE_SUFFIX).min(GAUGE_MAX);
    if cells == 0 || facts.memory_bytes() == 0 {
        return None;
    }
    let per_cell = facts.memory_bytes() as f64 / cells as f64;
    let others: i64 = facts
        .residents
        .iter()
        .filter(|resident| resident.id != record.id)
        .map(|resident| resident.bytes)
        .sum();
    let others = ((others.max(0) as f64 / per_cell).round() as usize).min(cells);
    let grown = f64::from(look.motion.eased(look.selected_at, GAUGE_GROW_MS));
    let mine = ((needed.max(0) as f64 / per_cell * grown).round() as usize)
        .max(1)
        .min(cells - others);
    let warm = facts.is_warm(&record.id);
    let gone = record.state == ModelState::Missing;
    let mine_colour = if gone {
        LINE
    } else if warm {
        INK_COLOR
    } else {
        mix(FAINT_COLOR, INK_COLOR, look.breath(3.0, 0.0))
    };
    let mut spans = vec![label("", label_column())];
    spans.push(Span::styled(
        "━".repeat(others),
        Style::new().fg(ACCENT_DIM),
    ));
    spans.push(Span::styled("━".repeat(mine), Style::new().fg(mine_colour)));
    spans.push(Span::styled(
        "━".repeat(cells - others - mine),
        Style::new().fg(LINE_STRONG),
    ));
    spans.push(Span::styled(
        if warm { "  held" } else { "  if warmed" },
        DIM,
    ));
    Some(Line::from(spans))
}

/// `path   ~/.ollama/…/sha256-ab12`, elided in the middle to `value_width`;
/// a path whose file is gone says so after it. Nothing for a record
/// without one.
fn path_line(record: &ModelRecord, value_width: usize) -> Option<Line<'static>> {
    let path = record.primary_weight_path.as_ref()?;
    let shown = text::at_home(path);
    let labels = label_column();
    if record.state != ModelState::Missing {
        return Some(Line::from(styled_field(
            "path",
            text::elide_middle(&shown, value_width),
            labels,
            SOFT,
        )));
    }
    let room = value_width.saturating_sub(GONE_SUFFIX.width());
    let mut spans = styled_field("path", text::elide_middle(&shown, room), labels, SOFT);
    spans.push(Span::styled(GONE_SUFFIX, DIM));
    Some(Line::from(spans))
}

/// `size   4.7 GB · ctx 32k`, whichever of the two the record knows, where the
/// size is what serving the model loads. When the store holds more than that
/// on disk (a repo with several quantizations), the disk figure follows:
/// `size   8.5 GB · ctx 32k · 34 GB on disk`.
fn size_line(record: &ModelRecord, value_width: usize) -> Line<'static> {
    let mut size = match (record.serving_size(), record.context_length) {
        (Some(bytes), Some(context)) => {
            format!("{} · ctx {}", text::bytes(bytes), text::tokens(context))
        }
        (Some(bytes), None) => text::bytes(bytes),
        (None, Some(context)) => format!("ctx {}", text::tokens(context)),
        (None, None) => DASH.to_owned(),
    };
    if let Some(disk) = record.size_on_disk()
        && record.serving_size() != Some(disk)
    {
        size = format!("{size} · {} on disk", text::bytes(disk));
    }
    row("size", &size, value_width, Style::new())
}

/// The last day of gateway traffic for the model: served requests, their
/// latency, and a bar per hour; when `expanded`, the hours are labelled on
/// a line of their own, else beside the bars.
fn activity_lines(
    activity: Option<&ModelActivity>,
    now: i64,
    expanded: bool,
    value_width: usize,
    look: &Look,
) -> Vec<Line<'static>> {
    let labels = label_column();
    let row = |label, value: String| field_line(label, text::clip(&value, value_width), labels);
    let absent = |label, value: String| {
        Line::from(styled_field(
            label,
            text::clip(&value, value_width),
            labels,
            DIM,
        ))
    };
    let Some(activity) = activity else {
        return vec![absent("last 24h", NO_GATEWAY_REQUESTS.to_owned())];
    };
    let mut lines = vec![row("last used", last_used(activity, now))];
    if activity.requests == 0 {
        lines.push(absent("last 24h", "no requests".to_owned()));
        return lines;
    }
    lines.push(row("last 24h", served(activity)));
    if let Some(latency) = &activity.latency {
        lines.push(row(
            "latency",
            format!(
                "p50 {}ms  p90 {}ms  p99 {}ms",
                latency.p50, latency.p90, latency.p99
            ),
        ));
    }
    let mut spark = vec![label("", labels)];
    spark.extend(sparkline(&activity.hourly, look));
    if !expanded && HOURS + SPARK_AXIS.width() <= value_width {
        spark.push(Span::styled(SPARK_AXIS, DIM));
    }
    lines.push(Line::from(spark));
    if expanded {
        lines.push(absent(
            "",
            format!("{:<width$}now", "24h ago", width = HOURS - 3),
        ));
    }
    lines
}

/// A bar per hour, oldest first, each rising a step after the one before
/// it when the selection changes; an empty hour is a low rule, the newest
/// hour the brightest bar.
pub(super) fn sparkline(hourly: &[u32], look: &Look) -> Vec<Span<'static>> {
    let highest = hourly.iter().copied().max().unwrap_or(0).max(1);
    let last = hourly.len().saturating_sub(1);
    hourly
        .iter()
        .enumerate()
        .map(|(hour, &count)| {
            let risen = look.motion.eased(
                look.selected_at + hour as u64 * SPARK_STEP_MS,
                SPARK_RISE_MS,
            );
            let level =
                ((f64::from(count) / f64::from(highest)) * 8.0 * f64::from(risen)).round() as usize;
            if count == 0 || level == 0 {
                return Span::styled("▁", Style::new().fg(LINE));
            }
            let colour = if hour == last {
                BRIGHT
            } else {
                mix(GHOST, SOFT_COLOR, level as f32 / 8.0)
            };
            Span::styled(LEVELS[level.min(8)], Style::new().fg(colour))
        })
        .collect()
}

/// The one line of gateway traffic the compact pane has room for: the last
/// day's requests, or when the model was last used if there were none.
fn activity_line(activity: Option<&ModelActivity>, now: i64, value_width: usize) -> Line<'static> {
    let labels = label_column();
    match activity {
        Some(activity) if activity.requests > 0 => field_line(
            "last 24h",
            text::clip(&served(activity), value_width),
            labels,
        ),
        Some(activity) => field_line(
            "last used",
            text::clip(&last_used(activity, now), value_width),
            labels,
        ),
        None => Line::from(styled_field(
            "last 24h",
            text::clip(NO_GATEWAY_REQUESTS, value_width),
            labels,
            DIM,
        )),
    }
}

/// `12 requests served`.
fn served(activity: &ModelActivity) -> String {
    format!(
        "{} served",
        text::count(activity.requests as usize, "request")
    )
}

/// `4m ago`, measured from `now`.
fn last_used(activity: &ModelActivity, now: i64) -> String {
    clock::duration((now - activity.last_seen_millis) / 1000) + " ago"
}

/// [`text::fit_parts`]' summary, then how much would be free with the rest of what
/// is loaded still in memory; a record whose weights are gone says so first.
fn fit_line(record: &ModelRecord, facts: &Facts) -> String {
    let (summary, fit) = text::fit_parts(record, &facts.machine);
    let summary = if record.state == ModelState::Missing {
        format!("weights are gone · {summary}")
    } else {
        summary
    };
    let Some(fit) = fit else {
        return summary;
    };
    let required_bytes = fit.assessment.required_bytes;
    let others: i64 = facts
        .residents
        .iter()
        .filter(|resident| resident.id != record.id)
        .map(|resident| resident.bytes)
        .sum();
    let free_after = fit.budget_bytes as i64 - others - required_bytes;
    let beside = if others == 0 {
        String::new()
    } else if free_after < 0 {
        format!(" · won't fit beside the {} GiB loaded", text::gib(others))
    } else {
        format!(" · {} GiB free beside what's loaded", text::gib(free_after))
    };
    format!("{summary}{beside}")
}

/// `residency   ● warm · gateway :11434 · unloads in 4m`, the holder clipped
/// to `value_width` cells with the state; the dot breathes when `look` lets
/// it move. While a task is under way on the model, a spinner says so.
fn residency_line(
    record: &ModelRecord,
    facts: &Facts,
    value_width: usize,
    look: Option<&Look>,
) -> Line<'static> {
    const WARM_LABEL: &str = "● warm";
    let mut spans = vec![label("residency", label_column())];
    if let Some(look) = look.filter(|look| look.busy) {
        spans.push(Span::styled(spinner(look.spin_frame).to_owned(), ACCENT));
        spans.push(Span::styled(" loading", INK));
        return Line::from(spans);
    }
    match facts.resident(&record.id) {
        Some(resident) => {
            let breath = look.map_or(1.0, |look| look.breath(2.1, 0.0));
            spans.push(Span::styled(
                "● ",
                Style::new().fg(mix(OLIVE_DIM, OLIVE, 0.6 + 0.4 * breath)),
            ));
            spans.push(Span::styled("warm", INK));
            let mut holder = match resident.holder {
                Holder::Local => " · this process".to_owned(),
                Holder::Daemon => " · Ollama daemon".to_owned(),
                Holder::Gateway => match facts.gateway_port {
                    Some(port) => format!(" · gateway :{port}"),
                    None => " · gateway".to_owned(),
                },
            };
            if let Some(seconds) = resident.expires_in_seconds_at(facts.collected_at_millis) {
                holder.push_str(&format!(" · unloads in {}", clock::duration(seconds)));
            }
            spans.push(Span::styled(
                text::clip(&holder, value_width.saturating_sub(WARM_LABEL.width())),
                SOFT,
            ));
        }
        None => spans.push(Span::styled("cold", DIM)),
    }
    Line::from(spans)
}

/// The pane's title: the model's name, escaped as the shelf table escapes
/// it, so a name cannot color the border or reorder it.
fn title(record: &ModelRecord) -> String {
    format!(" {} ", printable(record.display_name()))
}

#[cfg(test)]
mod tests;
