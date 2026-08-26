//! The detail pane: the selected record's facts, its residency, how it fits
//! beside what is already loaded, and what the gateway has served of it.

use kernel::profiles::{FitAssessment, FitVerdict};
use kernel::records::{Capability, ModelRecord};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use std::path::PathBuf;

use super::{ACCENT, BOLD, DIM, WARM, field, field_line, key_spans, styled_field};
use crate::support::residency::Holder;
use crate::support::shelf_table::{DASH, runtime_label, verdict_label};
use crate::tui::app::App;
use crate::tui::facts::{Facts, HOURS, ModelActivity};
use crate::tui::text;

/// Width of the labels in the pane: the longest, `runtime id`, plus a gap.
const LABEL_WIDTH: usize = 11;

/// Draw the detail pane into `area` for the selected model.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(record) = app.selected_record() else {
        let block = Block::bordered()
            .title(Span::styled(" detail ", DIM))
            .border_style(DIM);
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
    let mut lines = lines(
        record,
        &app.facts,
        &app.actions(),
        app.expanded,
        area.width.saturating_sub(2) as usize,
    );
    lines.truncate(area.height.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn lines(
    record: &ModelRecord,
    facts: &Facts,
    actions: &[(&str, &str)],
    expanded: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, LABEL_WIDTH);
    let eyebrow = |text: &'static str| Line::from(Span::styled(format!(" {text}"), ACCENT));
    let size = match (record.footprint_bytes(), record.context_length) {
        (Some(bytes), Some(context)) => {
            format!("{} · ctx {}", text::bytes(bytes), text::tokens(context))
        }
        (Some(bytes), None) => text::bytes(bytes),
        (None, Some(context)) => format!("ctx {}", text::tokens(context)),
        (None, None) => DASH.to_owned(),
    };
    let mut lines = vec![
        eyebrow("MODEL"),
        row(
            "runtime",
            text::short_runtime(runtime_label(record)).to_owned(),
        ),
        row(
            "store",
            text::short_store(record.source.kind.as_str()).to_owned(),
        ),
        row("size", size),
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
        residency_line(record, facts),
        actions_line(actions),
        Line::default(),
        eyebrow("GATEWAY"),
    ];
    lines.extend(activity_lines(
        facts.activity.for_record(record),
        facts.collected_at_millis,
        expanded,
    ));
    lines.push(Line::default());
    if let Some(path) = &record.primary_weight_path {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let shown = text::home_relative(path, home.as_deref());
        lines.push(row(
            "path",
            text::elide_middle(&shown, width.saturating_sub(LABEL_WIDTH + 2)),
        ));
    }
    if expanded {
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

/// `actions   w warm  l launch …`: the same verbs the footer offers, beside
/// the model on a wide screen.
fn actions_line(actions: &[(&str, &str)]) -> Line<'static> {
    let mut spans = field("actions", "", LABEL_WIDTH);
    if actions.is_empty() {
        spans.push(Span::styled("none right now", DIM));
    } else {
        spans.extend(key_spans(actions));
    }
    Line::from(spans)
}

/// The last day of gateway traffic for the model: served requests, their
/// latency, and a sparkline per hour; when expanded, the hours are labelled.
fn activity_lines(
    activity: Option<&ModelActivity>,
    now: i64,
    expanded: bool,
) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, LABEL_WIDTH);
    let absent = |label, value: String| Line::from(styled_field(label, value, LABEL_WIDTH, DIM));
    let Some(activity) = activity else {
        return vec![absent(
            "last 24h",
            "no requests through the gateway".to_owned(),
        )];
    };
    let mut lines = vec![row(
        "last used",
        text::duration((now - activity.last_seen_millis) / 1000) + " ago",
    )];
    if activity.requests == 0 {
        lines.push(absent("last 24h", "no requests".to_owned()));
        return lines;
    }
    lines.push(row(
        "last 24h",
        format!(
            "{} served",
            text::count(activity.requests as usize, "request")
        ),
    ));
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

/// `fits · 4.7 of 64 GiB`, then how much would be free with the rest of what
/// is loaded still in memory; `too big` when it never fits.
fn fit_line(record: &ModelRecord, facts: &Facts) -> String {
    let Some(FitAssessment {
        verdict,
        required_bytes,
    }) = FitVerdict::assess(record.footprint_mb, facts.memory_bytes)
    else {
        return "unknown footprint".to_owned();
    };
    if verdict == FitVerdict::TooLarge {
        return "too big for this machine".to_owned();
    }
    let verdict_label = verdict_label(Some(verdict));
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
    format!(
        "{verdict_label} · needs {} of {} GiB{beside}",
        text::gib(required_bytes),
        text::gib(facts.memory_bytes as i64)
    )
}

fn residency_line(record: &ModelRecord, facts: &Facts) -> Line<'static> {
    let mut spans = field("residency", "", LABEL_WIDTH);
    match facts.resident(&record.id) {
        Some(resident) => {
            spans.push(Span::styled("warm", WARM));
            let holder = match resident.holder {
                Holder::Local => " · this process".to_owned(),
                Holder::Daemon => " · Ollama daemon".to_owned(),
                Holder::Gateway => match facts.gateway_port {
                    Some(port) => format!(" · gateway :{port}"),
                    None => " · gateway".to_owned(),
                },
            };
            spans.push(Span::styled(holder, DIM));
            if let Some(seconds) = resident.expires_in_seconds() {
                spans.push(Span::styled(
                    format!(" · unloads in {}", text::duration(seconds)),
                    DIM,
                ));
            }
        }
        None => spans.push(Span::styled("cold", DIM)),
    }
    Line::from(spans)
}
