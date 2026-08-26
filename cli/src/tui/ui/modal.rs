//! The modals (pull, remove, help), drawn over a dimmed screen.

use kernel::install::plan::InstallPlan;
use kernel::profiles::FitVerdict;
use kernel::removal::ModelDeletionPreview;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::{ACCENT, BOLD, CURSOR, DIM, field_line, keys};
use crate::support::banner::KOALA;
use crate::support::harnesses::HARNESSES;
use crate::support::shelf_table::verdict_label;
use crate::tui::app::{App, Modal};
use crate::tui::launch::LaunchModal;
use crate::tui::pull::{PullMatch, PullModal, Stage, fit};
use crate::tui::text;

const WIDTH: u16 = 84;
const PULL_HEIGHT: u16 = 18;
const REMOVE_HEIGHT: u16 = 11;
const HELP_HEIGHT: u16 = 17;
/// The launch modal: a blank, one row per harness, a blank, the note, a
/// blank, the keys, and the border.
const LAUNCH_HEIGHT: u16 = HARNESSES.len() as u16 + 7;
/// Width of the labels in the preview and remove bodies.
const LABEL_WIDTH: usize = 10;
/// Width of the provider column: `huggingface` is the longest id.
const PROVIDER_WIDTH: usize = 11;
/// Width of the trailing fit verdict or popularity note.
const NOTE_WIDTH: usize = 14;
/// Rows of the listing that are not matches: the input, a blank, the keys.
const LISTING_CHROME_ROWS: usize = 3;

/// Draw the open modal over `area`, if there is one.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(modal) = &app.modal else {
        return;
    };
    frame.buffer_mut().set_style(area, DIM);
    let height = match modal {
        Modal::Pull(_) => PULL_HEIGHT,
        Modal::Remove(_) => REMOVE_HEIGHT,
        Modal::Help => HELP_HEIGHT,
        Modal::Launch(_) => LAUNCH_HEIGHT,
    };
    let rect = centered(area, height);
    let block = Block::bordered().border_style(ACCENT);
    let inner = block.inner(rect);
    let (title, lines) = match modal {
        Modal::Pull(modal) => (" pull ".to_owned(), pull(modal, app, inner)),
        Modal::Remove(preview) => (format!(" remove {} ", preview.name), remove(preview, app)),
        Modal::Help => (" help ".to_owned(), help()),
        Modal::Launch(modal) => (
            format!(" launch on {} ", modal.record.display_name()),
            launch(modal),
        ),
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(block.title(Span::styled(title, ACCENT)), rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Every harness, the ones this model can seat selectable, the rest dim with
/// the reason.
fn launch(modal: &LaunchModal) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    for (index, row) in modal.rows.iter().enumerate() {
        let mut line = Line::from(vec![
            Span::raw(format!(" {:<12}", row.spec.display)),
            Span::styled(format!("{:<10}", row.spec.binary), DIM),
            Span::styled(row.blocked.clone().unwrap_or_default(), DIM),
        ]);
        if row.blocked.is_some() {
            line = line.style(DIM);
        }
        if index == modal.selected {
            line = line.style(Style::new().add_modifier(Modifier::REVERSED));
        }
        lines.push(line);
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        " the ui steps aside while the harness runs, and comes back when it exits",
        DIM,
    )));
    lines.push(Line::default());
    lines.push(keys(&[
        ("enter", "launch"),
        ("↑/↓", "move"),
        ("esc", "close"),
    ]));
    lines
}

/// The key table beside the koala, and the one idea behind it.
fn help() -> Vec<Line<'static>> {
    const ROWS: [(&str, &str, &str, &str); 8] = [
        ("j k ↑ ↓", "move", "g G", "top / bottom"),
        ("/", "filter", "enter", "expand detail"),
        ("p", "pull", "s", "scan"),
        ("w u", "warm / unload", "x", "remove"),
        ("l", "launch a harness", "o", "sort"),
        ("r", "refresh", "", ""),
        ("y Y", "copy path / id", "c", "cancel pull"),
        ("d", "dismiss failure", "q", "quit"),
    ];
    const BLANK: (&str, &str, &str, &str) = ("", "", "", "");
    let mut lines = vec![Line::default()];
    for (koala, (key, verb, key2, verb2)) in KOALA
        .iter()
        .zip(ROWS.iter().chain(std::iter::repeat(&BLANK)))
    {
        lines.push(Line::from(vec![
            Span::styled(format!("  {koala}   "), BOLD),
            Span::styled(format!("{key:<8}"), DIM),
            Span::raw(format!("{verb:<16}")),
            Span::styled(format!("{key2:<8}"), DIM),
            Span::raw(format!("{verb2:<16}")),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  every key is a hedos subcommand: p is pull, x is rm, w is warm.",
        DIM,
    )));
    lines.push(Line::default());
    lines.push(keys(&[("any key", "close")]));
    lines
}

/// What removing the model does, in the store's own terms.
fn remove(preview: &ModelDeletionPreview, app: &App) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, LABEL_WIDTH);
    let what = if preview.via_daemon {
        "removes the tag through the Ollama daemon (ollama rm)".to_owned()
    } else if preview.paths.is_empty() {
        "nothing is left on disk; this forgets the record".to_owned()
    } else {
        format!(
            "deletes {} permanently, not to the trash",
            text::count(preview.paths.len(), "path")
        )
    };
    let mut lines = vec![
        Line::default(),
        row("store", preview.kind.as_str().to_owned()),
        row("on disk", text::bytes(preview.bytes_estimate)),
    ];
    if let Some(path) = preview.paths.first() {
        lines.push(row("path", path.clone()));
    }
    lines.push(Line::default());
    lines.push(Line::from(format!(" {what}")));
    lines.push(Line::from(Span::styled(
        format!(
            " after: {} on disk",
            text::bytes((app.facts.disk_bytes() - preview.bytes_estimate).max(0))
        ),
        DIM,
    )));
    lines.push(Line::default());
    lines.push(keys(&[("y", "remove"), ("n", "keep")]));
    lines
}

/// The pull modal's body for its current stage.
fn pull(modal: &PullModal, app: &App, inner: Rect) -> Vec<Line<'static>> {
    match &modal.stage {
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
    }
}

fn preview(plan: &InstallPlan, app: &App) -> Vec<Line<'static>> {
    let memory = app.facts.memory_bytes;
    let row = |label, value: String| field_line(label, value, LABEL_WIDTH);
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
        row(
            "from",
            format!("{} · {}", plan.provider.as_str(), plan.reference),
        ),
        row("to", plan.destination.clone()),
        row("size", size),
        row("download", download),
        row(
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
            Span::styled(CURSOR, BOLD),
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

fn centered(area: Rect, height: u16) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height)),
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
