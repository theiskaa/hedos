//! The pull modal, drawn over a dimmed screen.

use kernel::install::plan::InstallPlan;
use kernel::profiles::FitVerdict;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::{BOLD, DIM, keys};
use crate::support::shelf_table::verdict_label;
use crate::tui::app::App;
use crate::tui::pull::{PullMatch, PullModal, Stage, fit};
use crate::tui::text;

const WIDTH: u16 = 84;
const HEIGHT: u16 = 18;
/// Width of the provider column: `huggingface` is the longest id.
const PROVIDER_WIDTH: usize = 11;
/// Width of the trailing fit verdict or popularity note.
const NOTE_WIDTH: usize = 14;
/// Rows of the listing that are not matches: the input, a blank, the keys.
const LISTING_CHROME_ROWS: usize = 3;

/// Draw the pull modal over `area` when it is open.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(modal) = &app.pull else {
        return;
    };
    frame.buffer_mut().set_style(area, DIM);
    let rect = centered(area);
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(" pull ").border_style(DIM);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let lines = match &modal.stage {
        Stage::Listing => listing(modal, app.facts.memory_bytes, inner),
        Stage::Planning(reference) => vec![
            Line::default(),
            Line::from(vec![
                Span::raw(format!(" resolving {reference}")),
                Span::styled("…", DIM),
            ]),
            Line::default(),
            keys(&[("esc", "back")]),
        ],
        Stage::Preview(plan) => preview(plan, app),
        Stage::Note(note) => vec![
            Line::default(),
            Line::from(format!(" {note}")),
            Line::default(),
            keys(&[("esc", "back")]),
        ],
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn preview(plan: &InstallPlan, app: &App) -> Vec<Line<'static>> {
    let memory = app.facts.memory_bytes;
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::styled(format!(" {label:<9} "), DIM),
            Span::raw(value),
        ])
    };
    let size = match plan.total_bytes {
        Some(total) => format!(
            "{} · {} when warm",
            text::bytes(total),
            verdict_label(fit(Some(total), memory))
        ),
        None => "size unknown".to_owned(),
    };
    let download = match (plan.remaining_bytes, plan.total_bytes) {
        (Some(0), Some(_)) => "already on disk".to_owned(),
        (Some(remaining), Some(total)) if remaining < total => {
            format!("{} of that", text::bytes(remaining))
        }
        _ => "all of it".to_owned(),
    };
    vec![
        Line::default(),
        Line::from(Span::styled(format!(" {}", plan.display_name), BOLD)),
        Line::default(),
        field(
            "from",
            format!("{} · {}", plan.provider.as_str(), plan.reference),
        ),
        field("to", plan.destination.clone()),
        field("size", size),
        field("download", download),
        field(
            "after",
            format!(
                "{} on disk",
                text::bytes(app.facts.disk_bytes() + plan.remaining_bytes.unwrap_or(0))
            ),
        ),
        Line::default(),
        keys(&[("enter", "pull"), ("esc", "back")]),
    ]
}

fn listing(modal: &PullModal, memory_bytes: u64, inner: Rect) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" › ", BOLD),
            Span::raw(modal.input.clone()),
            Span::styled("▏", BOLD),
        ]),
        Line::default(),
    ];
    let rows = (inner.height as usize).saturating_sub(LISTING_CHROME_ROWS);
    let first = modal.selected.saturating_sub(rows.saturating_sub(1));
    for (index, candidate) in modal.matches.iter().enumerate().skip(first).take(rows) {
        let mut line = row(candidate, memory_bytes, inner.width);
        if index == modal.selected {
            line = line.style(Style::new().add_modifier(Modifier::REVERSED));
        }
        lines.push(line);
    }
    if modal.matches.is_empty() {
        lines.push(Line::from(Span::styled(
            " type a name, owner/repo, or name:tag",
            DIM,
        )));
    }
    while lines.len() + 1 < inner.height as usize {
        lines.push(Line::default());
    }
    lines.push(keys(&[
        ("enter", "choose"),
        ("↑/↓", "move"),
        ("esc", "close"),
    ]));
    lines
}

/// `provider  reference  size  fit`, the reference trimmed and the note
/// clipped so the columns hold.
fn row(candidate: &PullMatch, memory_bytes: u64, width: u16) -> Line<'static> {
    let verdict = candidate.fit(memory_bytes);
    let (size, note) = match candidate.bytes {
        Some(bytes) => (text::bytes(bytes), verdict_label(verdict).to_owned()),
        None => (String::new(), candidate.note.clone()),
    };
    let note: String = note.chars().take(NOTE_WIDTH).collect();
    let tail = format!("{size:>8}  {note:<NOTE_WIDTH$}");
    let head_width = (width as usize).saturating_sub(tail.chars().count() + PROVIDER_WIDTH + 3);
    let reference: String = candidate.reference.chars().take(head_width).collect();
    let style = if verdict == Some(FitVerdict::TooLarge) {
        DIM
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(
            format!(" {:<PROVIDER_WIDTH$} ", candidate.provider.as_str()),
            DIM,
        ),
        Span::raw(format!("{reference:<head_width$} ")),
        Span::styled(tail, style),
    ])
}

fn centered(area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(HEIGHT.min(area.height)),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, rect, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(WIDTH.min(area.width)),
        Constraint::Fill(1),
    ])
    .areas(middle);
    rect
}
