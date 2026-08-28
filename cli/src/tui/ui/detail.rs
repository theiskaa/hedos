//! The detail pane: the selected record's facts, its residency, how it fits
//! beside what is already loaded, and what the gateway has served of it.

use kernel::profiles::{FitAssessment, FitVerdict};
use kernel::records::{Capability, ModelRecord, ModelState};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use unicode_width::UnicodeWidthStr;

use super::{
    ACCENT, BOLD, DIM, EYEBROW, WARM, field_line, label, label_width, styled_field, value_width,
};
use crate::support::residency::Holder;
use crate::support::shelf_table::{DASH, runtime_label, verdict_label};
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

/// How much of the pane there is: the stacked pane's height decides first,
/// then whether the user has expanded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    /// The stacked pane, four rows: what the shelf row does not already show.
    Compact,
    /// The facts, memory, gateway, and path.
    Full,
    /// `Full`, the sparkline's hours labelled, and the record's identifiers.
    Expanded,
}

/// The width of the label column.
fn label_column() -> usize {
    label_width(&LABELS, 1)
}

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
    let detail = if area.height <= STACKED_DETAIL_ROWS {
        Depth::Compact
    } else if app.expanded {
        Depth::Expanded
    } else {
        Depth::Full
    };
    let lines = lines(
        record,
        &app.facts,
        detail,
        area.width.saturating_sub(2) as usize,
    );
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// What the pane appends to a path whose file is no longer there.
const GONE_SUFFIX: &str = " · gone";

/// The pane's rows at `width` cells: a value that would run past the edge is
/// clipped, a path elided in the middle. Compact is always four rows, the
/// size standing in for a path the record does not have.
fn lines(record: &ModelRecord, facts: &Facts, detail: Depth, width: usize) -> Vec<Line<'static>> {
    let labels = label_column();
    let value_width = value_width(width, labels);
    let row = |label, value: String| field_line(label, text::clip(&value, value_width), labels);
    let eyebrow = |text: &'static str| Line::from(Span::styled(format!(" {text}"), EYEBROW));
    let path_line = || {
        record.primary_weight_path.as_ref().map(|path| {
            let shown = text::at_home(path);
            if record.state != ModelState::Missing {
                return field_line("path", text::elide_middle(&shown, value_width), labels);
            }
            let room = value_width.saturating_sub(GONE_SUFFIX.width());
            let mut spans = styled_field(
                "path",
                text::elide_middle(&shown, room),
                labels,
                Style::new(),
            );
            spans.push(Span::styled(GONE_SUFFIX, DIM));
            Line::from(spans)
        })
    };
    let size = match (record.footprint_bytes(), record.context_length) {
        (Some(bytes), Some(context)) => {
            format!("{} · ctx {}", text::bytes(bytes), text::tokens(context))
        }
        (Some(bytes), None) => text::bytes(bytes),
        (None, Some(context)) => format!("ctx {}", text::tokens(context)),
        (None, None) => DASH.to_owned(),
    };
    if detail == Depth::Compact {
        let mut lines = vec![
            row("fit", fit_line(record, facts)),
            residency_line(record, facts, value_width),
            activity_line(
                facts.activity.for_record(record),
                facts.collected_at_millis,
                value_width,
            ),
        ];
        lines.push(path_line().unwrap_or_else(|| row("size", size)));
        return lines;
    }
    let mut lines = vec![
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
        residency_line(record, facts, value_width),
        Line::default(),
        eyebrow("GATEWAY"),
    ];
    lines.extend(activity_lines(
        facts.activity.for_record(record),
        facts.collected_at_millis,
        detail,
        value_width,
    ));
    lines.push(Line::default());
    lines.extend(path_line());
    if detail == Depth::Expanded {
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

/// The last day of gateway traffic for the model: served requests, their
/// latency, and a sparkline per hour; when expanded, the hours are labelled.
fn activity_lines(
    activity: Option<&ModelActivity>,
    now: i64,
    detail: Depth,
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
    if detail == Depth::Expanded {
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

/// `fits · needs 4.7 of 64 GiB`, `too big for this machine`, or that the
/// footprint is unknown: the shape the detail and the pull preview share.
pub(super) fn fit_summary(footprint_mb: Option<i64>, memory_bytes: u64) -> String {
    fit_parts(footprint_mb, memory_bytes).0
}

/// [`fit_summary`] and, when the model fits at all, the bytes it needs.
fn fit_parts(footprint_mb: Option<i64>, memory_bytes: u64) -> (String, Option<i64>) {
    match FitVerdict::assess(footprint_mb, memory_bytes) {
        None => ("unknown footprint".to_owned(), None),
        Some(FitAssessment {
            verdict: FitVerdict::TooLarge,
            ..
        }) => ("too big for this machine".to_owned(), None),
        Some(FitAssessment {
            verdict,
            required_bytes,
        }) => (
            format!(
                "{} · needs {} of {} GiB",
                verdict_label(Some(verdict)),
                text::gib(required_bytes),
                text::gib(memory_bytes as i64)
            ),
            Some(required_bytes),
        ),
    }
}

/// [`fit_summary`], then how much would be free with the rest of what is
/// loaded still in memory; a record whose weights are gone says so first.
fn fit_line(record: &ModelRecord, facts: &Facts) -> String {
    let (summary, required_bytes) = fit_parts(record.footprint_mb, facts.memory_bytes);
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
            if let Some(seconds) = resident.expires_in_seconds() {
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
mod tests {
    use super::*;

    use crate::tui::facts::ModelActivity;
    use crate::tui::testing::{facts_with_memory, record_with, resident_with_bytes, text, texts};
    use crate::tui::ui::leading_label;
    use gateway::stats::LatencyPercentiles;

    #[test]
    fn every_label_is_listed() {
        let mut record = record_with("m", vec![Capability::chat()]);
        record.alias = Some("alias".to_owned());
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        let mut facts = Facts {
            collected_at_millis: 1_000_000,
            ..facts_with_memory(64)
        };
        facts.activity.models.insert(
            record.id.clone(),
            ModelActivity {
                requests: 3,
                latency: Some(LatencyPercentiles {
                    p50: 1,
                    p90: 2,
                    p99: 3,
                }),
                hourly: [0; HOURS],
                last_seen_millis: 500_000,
            },
        );
        let mut seen = 0;
        for line in lines(&record, &facts, Depth::Expanded, 80) {
            let label = leading_label(&line, label_column());
            if label.is_empty() || label.chars().all(|c| c.is_uppercase()) {
                continue;
            }
            assert!(LABELS.contains(&label.as_str()), "{label} is not listed");
            seen += 1;
        }
        assert_eq!(seen, LABELS.len());
    }

    #[test]
    fn a_gone_record_says_so_on_path_and_fit() {
        let mut record = record_with("m", vec![Capability::chat()]);
        record.footprint_mb = Some(4 * 1024);
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        record.state = ModelState::Missing;
        let facts = facts_with_memory(64);
        let lines = lines(&record, &facts, Depth::Full, 80);
        let path = lines
            .iter()
            .find(|line| text(line).starts_with(" path"))
            .expect("a path row");
        assert!(
            text(path).ends_with("/models/m.gguf · gone"),
            "{:?}",
            text(path)
        );
        assert_eq!(path.spans.last().map(|span| span.style), Some(DIM));
        let fit = lines
            .iter()
            .map(text)
            .find(|line| line.starts_with(" fit"))
            .unwrap_or_default();
        assert!(fit.contains("weights are gone · fits · needs"), "{fit:?}");
    }

    #[test]
    fn long_values_are_clipped_to_the_pane() {
        let caps = [
            "chat",
            "complete",
            "embed",
            "see",
            "image",
            "speak",
            "transcribe",
            "tools",
        ];
        let mut record = record_with("m", caps.into_iter().map(Capability::from).collect());
        record.footprint_mb = Some(4 * 1024);
        record.primary_weight_path = Some(format!("/models/{}.gguf", "x".repeat(80)));
        let mut gateway = resident_with_bytes(&record.id, Holder::Gateway, 4 << 30);
        gateway.expires_at_millis = Some(i64::MAX / 2);
        let facts = Facts {
            gateway_port: Some(11434),
            residents: vec![
                gateway,
                resident_with_bytes("other", Holder::Local, 30 << 30),
            ],
            ..facts_with_memory(64)
        };
        let lines = lines(&record, &facts, Depth::Expanded, 40);
        for line in &lines {
            assert!(line.width() <= 40, "{:?} runs past the pane", text(line));
        }
        let find = |label: &str| {
            lines
                .iter()
                .map(text)
                .find(|line| line.starts_with(&format!(" {label}")))
                .unwrap_or_default()
        };
        assert!(find("caps").ends_with('…'));
        assert!(find("fit").ends_with('…'));
        assert!(find("residency").contains("warm") && find("residency").ends_with('…'));
        assert!(find("path").contains('…') && find("path").ends_with(".gguf"));
        assert!(texts(&lines).contains(&" RECORD".to_owned()));
    }

    #[test]
    fn the_compact_detail_skips_what_the_row_shows() {
        let mut record = record_with("m", vec![Capability::chat()]);
        record.footprint_mb = Some(4 * 1024);
        record.primary_weight_path = Some("/models/m.gguf".to_owned());
        let mut facts = Facts {
            collected_at_millis: 1_000_000,
            ..facts_with_memory(64)
        };
        let labels_of = |lines: &[Line]| -> Vec<String> {
            lines
                .iter()
                .map(|line| leading_label(line, label_column()))
                .collect()
        };
        let quiet = lines(&record, &facts, Depth::Compact, 80);
        assert_eq!(labels_of(&quiet), ["fit", "residency", "last 24h", "path"]);
        assert!(text(&quiet[2]).contains("no requests through the gateway"));

        facts.activity.models.insert(
            record.id.clone(),
            ModelActivity {
                requests: 0,
                latency: None,
                hourly: [0; HOURS],
                last_seen_millis: 500_000,
            },
        );
        let idle = lines(&record, &facts, Depth::Compact, 80);
        assert_eq!(labels_of(&idle), ["fit", "residency", "last used", "path"]);
        assert!(text(&idle[2]).ends_with("ago"));

        facts.activity.models.get_mut(&record.id).unwrap().requests = 12;
        let busy = lines(&record, &facts, Depth::Compact, 80);
        assert_eq!(labels_of(&busy)[2], "last 24h");
        assert!(text(&busy[2]).contains("12 requests served"));

        record.primary_weight_path = None;
        let pathless = lines(&record, &facts, Depth::Compact, 80);
        assert_eq!(
            labels_of(&pathless),
            ["fit", "residency", "last 24h", "size"]
        );
        assert!(text(&pathless[3]).contains("4 GB"));

        let full = lines(&record, &facts, Depth::Full, 80);
        assert!(labels_of(&full).contains(&"runtime".to_owned()));
        assert!(full.len() > STACKED_DETAIL_ROWS as usize);
        assert!(!texts(&full).contains(&" RECORD".to_owned()));
    }
}
