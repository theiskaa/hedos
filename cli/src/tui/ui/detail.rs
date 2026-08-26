//! The detail pane: the selected record's facts, its residency, and how it
//! fits beside what is already loaded.

use kernel::profiles::{FitAssessment, FitVerdict};
use kernel::records::byte_format::BYTES_PER_MIB;
use kernel::records::{Capability, ModelRecord};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{BOLD, DIM, field};
use crate::support::shelf_table::{DASH, runtime_label};
use crate::tui::app::App;
use crate::tui::facts::{Facts, Holder};
use crate::tui::text;

/// Width of the labels in the pane.
const LABEL_WIDTH: usize = 10;

/// Draw the detail pane into `area` for the selected model.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(record) = app.selected_record() else {
        let block = Block::bordered()
            .title(Span::styled(" detail ", DIM))
            .border_style(DIM);
        frame.render_widget(block, area);
        return;
    };
    let block = Block::bordered()
        .title(Span::styled(format!(" {} ", record.display_name()), BOLD))
        .border_style(DIM);
    let mut lines = lines(record, &app.facts);
    lines.truncate(area.height.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn lines(record: &ModelRecord, facts: &Facts) -> Vec<Line<'static>> {
    let row = |label, value: String| Line::from(field(label, value, LABEL_WIDTH));
    let or_dash = |value: Option<String>| value.unwrap_or_else(|| DASH.to_owned());
    vec![
        row("runtime", runtime_label(record).to_owned()),
        row("store", record.source.kind.as_str().to_owned()),
        row("path", or_dash(record.primary_weight_path.clone())),
        row(
            "on disk",
            or_dash(
                record
                    .footprint_mb
                    .map(|mb| text::bytes(mb * BYTES_PER_MIB)),
            ),
        ),
        row(
            "context",
            or_dash(record.context_length.map(|tokens| tokens.to_string())),
        ),
        row("fit", fit_line(record, facts)),
        row(
            "caps",
            record
                .capabilities
                .iter()
                .map(Capability::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        row("modality", record.modality.as_str().to_owned()),
        Line::default(),
        residency_line(record, facts),
    ]
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
    let verdict_label = match verdict {
        FitVerdict::RunsWell => "fits",
        FitVerdict::TightFit => "tight",
        FitVerdict::TooLarge => return "too big for this machine".to_owned(),
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
            spans.push(Span::styled("warm", BOLD));
            let holder = match resident.holder {
                Holder::Local => " · in this process".to_owned(),
                Holder::Gateway => match facts.gateway_port {
                    Some(port) => format!(" · held by the gateway on :{port}"),
                    None => " · held by the gateway".to_owned(),
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
