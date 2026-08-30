//! The detail pane: the selected record's facts, its residency, how it fits
//! beside what is already loaded, and what the gateway has served of it.

use kernel::records::{Capability, ModelRecord, ModelState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use unicode_width::UnicodeWidthStr;

use super::{
    ACCENT, BOLD, BORDER_COLUMNS, COOL, DIM, EYEBROW, WARM, field_line, label, label_width, pane,
    styled_field, value_width,
};
use crate::support::residency::Holder;
use crate::support::shelf_table::{DASH, runtime_label};
use crate::tui::app::App;
use crate::tui::facts::{Facts, HOURS, ModelActivity};
use crate::tui::layout::STACKED_DETAIL_ROWS;
use crate::tui::text;

/// The labels the pane uses; the column is as wide as the widest, plus a gap.
const LABELS: [&str; 17] = [
    "runtime",
    "store",
    "size",
    "caps",
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

/// The width of the label column.
fn label_column() -> usize {
    label_width(&LABELS, 1)
}

/// Draw the detail pane into `area` for the selected model.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(record) = app.selected_record() else {
        let block = pane(" detail ");
        frame.render_widget(block, area);
        return;
    };
    // Expanded, the pane is a mode of its own: the accent says so.
    let (title_style, border_style) = if app.expanded {
        (ACCENT, ACCENT)
    } else {
        (BOLD, DIM)
    };
    let block = Block::bordered()
        .title(Span::styled(
            format!(" {} ", record.display_name()),
            title_style,
        ))
        .border_style(border_style);
    // The stacked pane's height decides first, then whether the user has
    // expanded the pane.
    let width = area.width.saturating_sub(BORDER_COLUMNS) as usize;
    let lines = if area.height <= STACKED_DETAIL_ROWS {
        compact_lines(record, &app.facts, width)
    } else {
        full_lines(record, &app.facts, app.expanded, width)
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
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
        residency_line(record, facts, value_width),
        activity_line(
            facts.activity.for_record(record),
            facts.collected_at_millis,
            value_width,
        ),
        path_line(record, value_width).unwrap_or_else(|| size_line(record, value_width)),
    ]
}

/// The pane's rows at `width` cells: the facts, memory, gateway, and path,
/// and when `expanded` the sparkline's hours labelled and the record's
/// identifiers under them. A value that would run past the edge is clipped,
/// a path elided in the middle.
fn full_lines(
    record: &ModelRecord,
    facts: &Facts,
    expanded: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let value_width = value_width(width, label_column());
    let row = |label, value: String| row(label, &value, value_width, Style::new());
    // The runtime and the store wear the cool hue, as the shelf row shows them.
    let cool_row = |label, value: String| self::row(label, &value, value_width, COOL);
    let eyebrow = |text: &'static str| Line::from(Span::styled(format!(" {text}"), EYEBROW));
    let mut lines = vec![
        cool_row(
            "runtime",
            text::short_runtime(runtime_label(record)).to_owned(),
        ),
        cool_row(
            "store",
            text::short_store(record.source.kind.as_str()).to_owned(),
        ),
        size_line(record, value_width),
        row(
            "caps",
            record
                .capabilities
                .iter()
                .map(Capability::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Line::default(),
        eyebrow("MEMORY"),
        row("fit", fit_line(record, facts)),
        residency_line(record, facts, value_width),
        Line::default(),
        eyebrow("GATEWAY"),
    ];
    lines.extend(activity_lines(
        facts.activity.for_record(record),
        facts.collected_at_millis,
        expanded,
        value_width,
    ));
    lines.push(Line::default());
    lines.extend(path_line(record, value_width));
    if expanded {
        lines.push(Line::default());
        lines.push(eyebrow("RECORD"));
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
    lines
}

/// `path   ~/.ollama/…/sha256-ab12`, elided in the middle to `value_width`;
/// a path whose file is gone says so after it. Nothing for a record
/// without one.
fn path_line(record: &ModelRecord, value_width: usize) -> Option<Line<'static>> {
    let path = record.primary_weight_path.as_ref()?;
    let shown = text::at_home(path);
    let labels = label_column();
    if record.state != ModelState::Missing {
        return Some(field_line(
            "path",
            text::elide_middle(&shown, value_width),
            labels,
        ));
    }
    let room = value_width.saturating_sub(GONE_SUFFIX.width());
    let mut spans = styled_field(
        "path",
        text::elide_middle(&shown, room),
        labels,
        Style::new(),
    );
    spans.push(Span::styled(GONE_SUFFIX, DIM));
    Some(Line::from(spans))
}

/// `size   4.7 GB · ctx 32k`, whichever of the two the record knows.
fn size_line(record: &ModelRecord, value_width: usize) -> Line<'static> {
    let size = match (record.footprint_bytes(), record.context_length) {
        (Some(bytes), Some(context)) => {
            format!("{} · ctx {}", text::bytes(bytes), text::tokens(context))
        }
        (Some(bytes), None) => text::bytes(bytes),
        (None, Some(context)) => format!("ctx {}", text::tokens(context)),
        (None, None) => DASH.to_owned(),
    };
    row("size", &size, value_width, Style::new())
}

/// The last day of gateway traffic for the model: served requests, their
/// latency, and a sparkline per hour; when `expanded`, the hours are
/// labelled.
fn activity_lines(
    activity: Option<&ModelActivity>,
    now: i64,
    expanded: bool,
    value_width: usize,
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
    lines.push(absent("", text::sparkline(&activity.hourly)));
    if expanded {
        lines.push(absent(
            "",
            format!("{:<width$}now", "24h ago", width = HOURS - 3),
        ));
    }
    lines
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
    text::duration((now - activity.last_seen_millis) / 1000) + " ago"
}

/// [`text::fit_summary`], then how much would be free with the rest of what
/// is loaded still in memory; a record whose weights are gone says so first.
fn fit_line(record: &ModelRecord, facts: &Facts) -> String {
    let (summary, required_bytes) = text::fit_parts(record.footprint_mb, facts.memory_bytes);
    let summary = if record.state == ModelState::Missing {
        format!("weights are gone · {summary}")
    } else {
        summary
    };
    let Some(required_bytes) = required_bytes else {
        return summary;
    };
    let others: i64 = facts
        .residents
        .iter()
        .filter(|resident| resident.id != record.id)
        .map(|resident| resident.bytes)
        .sum();
    let free_after = facts.memory_bytes as i64 - others - required_bytes;
    let beside = if others == 0 {
        String::new()
    } else if free_after < 0 {
        format!(" · won't fit beside the {} GiB loaded", text::gib(others))
    } else {
        format!(" · {} GiB free beside what's loaded", text::gib(free_after))
    };
    format!("{summary}{beside}")
}

/// `residency   warm · gateway :11434 · unloads in 4m`, the holder clipped to
/// `value_width` cells with the state.
fn residency_line(record: &ModelRecord, facts: &Facts, value_width: usize) -> Line<'static> {
    const WARM_LABEL: &str = "warm";
    let mut spans = vec![label("residency", label_column())];
    match facts.resident(&record.id) {
        Some(resident) => {
            spans.push(Span::styled(WARM_LABEL, WARM));
            let mut holder = match resident.holder {
                Holder::Local => " · this process".to_owned(),
                Holder::Daemon => " · Ollama daemon".to_owned(),
                Holder::Gateway => match facts.gateway_port {
                    Some(port) => format!(" · gateway :{port}"),
                    None => " · gateway".to_owned(),
                },
            };
            if let Some(seconds) = resident.expires_in_seconds_at(facts.collected_at_millis) {
                holder.push_str(&format!(" · unloads in {}", text::duration(seconds)));
            }
            spans.push(Span::styled(
                text::clip(&holder, value_width.saturating_sub(WARM_LABEL.width())),
                DIM,
            ));
        }
        None => spans.push(Span::styled("cold", DIM)),
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests;
