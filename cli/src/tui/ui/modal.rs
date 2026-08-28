//! The modals (pull, remove, help, launch), drawn over a dimmed screen; the
//! chat pane, though it sits in the same slot, is drawn by `chat` instead.

use kernel::install::plan::InstallPlan;
use kernel::profiles::FitVerdict;
use kernel::removal::ModelDeletionPreview;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use super::{
    ACCENT, BACKDROP, BOLD, DIM, EYEBROW, SELECTED_ROW, centered, edited, field_line, keys,
    label_width, padded, right_aligned, styled_field,
};
use crate::support::harnesses::HARNESSES;
use crate::support::shelf_table::verdict_label;
use crate::tui::app::{App, Modal};
use crate::tui::facts::Facts;
use crate::tui::keymap;
use crate::tui::launch::LaunchModal;
use crate::tui::pull::{CATEGORIES, ListingRow, MAX_MATCHES, PullMatch, PullModal, Stage, fit};
use crate::tui::text;

/// The pull modal's width: the listing needs it for its reference column.
const PULL_WIDTH: u16 = 84;
/// The remove modal's width: three label rows and one sentence, the path
/// elided to fit.
const REMOVE_WIDTH: u16 = 72;
/// The launch modal's width: a harness, its binary, and the reason it is
/// blocked, clipped to fit.
const LAUNCH_WIDTH: u16 = 72;
/// The help modal's width with three columns of keys and verbs; two on a
/// narrow terminal, the card is as wide as the columns.
const HELP_WIDTH: u16 = 72;
/// Cells kept clear on either side of a modal when the terminal is narrower
/// than it wants.
const MARGIN: u16 = 2;
/// Rows a bordered box spends on its top and bottom edges.
const BORDER_ROWS: u16 = 2;
/// Columns a bordered box spends on its left and right edges.
const BORDER_COLUMNS: u16 = 2;
/// Rows of the listing that are not matches: the input, a blank, the keys.
const LISTING_CHROME_ROWS: usize = 3;
/// The pull modal: the border, the input and a blank, every match with an
/// eyebrow per category, and the keys.
const PULL_HEIGHT: u16 =
    (MAX_MATCHES + CATEGORIES.len() + LISTING_CHROME_ROWS) as u16 + BORDER_ROWS;
/// The remove modal: a blank, three rows, a blank, what happens and what is
/// left, a blank, the keys, and the border.
const REMOVE_HEIGHT: u16 = 9 + BORDER_ROWS;
/// The launch modal: a blank, one row per harness, a blank, the note, a
/// blank, the keys, and the border.
const LAUNCH_HEIGHT: u16 = HARNESSES.len() as u16 + 5 + BORDER_ROWS;
/// The labels of the preview and remove bodies; the column is as wide as
/// the widest, plus a gap.
const LABELS: [&str; 8] = [
    "store", "on disk", "path", "after", "from", "to", "size", "download",
];
/// Cells the size column of a pull row holds: `999.9 GB` at the widest.
const SIZE_WIDTH: usize = 8;
/// Cells the trailing fit verdict or popularity note is held to.
const NOTE_WIDTH: usize = 14;
/// The prompt marker of the pull query.
const MARK: &str = " › ";

/// Draw the open modal over `area`, if there is one.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(modal) = &app.modal else {
        return;
    };
    // The chat pane is a body of its own, not something over the shelf.
    if matches!(modal, Modal::Chat(_)) {
        return;
    }
    frame.buffer_mut().set_style(area, BACKDROP);
    let (width, height) = match modal {
        Modal::Pull(_) => (PULL_WIDTH, PULL_HEIGHT),
        Modal::Remove(_) => (REMOVE_WIDTH, REMOVE_HEIGHT),
        Modal::Help => (help_width(area.width), help_height(area.width)),
        Modal::Launch(_) => (LAUNCH_WIDTH, LAUNCH_HEIGHT),
        Modal::Chat(_) => return,
    };
    let rect = centered(area, modal_width(width, area.width), height);
    let block = Block::bordered().border_style(DIM);
    let inner = block.inner(rect);
    let (title, lines) = match modal {
        Modal::Pull(modal) => (" pull ".to_owned(), pull(modal, app, inner)),
        Modal::Remove(preview) => (
            format!(" remove {} ", preview.name),
            remove(preview, &app.facts, inner),
        ),
        Modal::Help => (" help ".to_owned(), help(area.width)),
        Modal::Launch(modal) => (
            format!(" launch on {} ", modal.record.display_name()),
            launch(modal, inner),
        ),
        Modal::Chat(_) => return,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(block.title(Span::styled(title, ACCENT)), rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `wanted` cells, or what `available` leaves once a margin is kept on both
/// sides.
fn modal_width(wanted: u16, available: u16) -> u16 {
    wanted.min(available.saturating_sub(2 * MARGIN))
}

/// Every harness, the ones this model can seat selectable, the rest dim with
/// the reason, clipped to what `inner` leaves after the two columns.
fn launch(modal: &LaunchModal, inner: Rect) -> Vec<Line<'static>> {
    let displays: Vec<&str> = HARNESSES.iter().map(|spec| spec.display).collect();
    let binaries: Vec<&str> = HARNESSES.iter().map(|spec| spec.binary).collect();
    let display_width = label_width(&displays, 1);
    let binary_width = label_width(&binaries, 2);
    let reason_width = (inner.width as usize).saturating_sub(1 + display_width + binary_width);
    let mut lines = vec![Line::default()];
    for (index, row) in modal.rows.iter().enumerate() {
        let reason = row.blocked.as_deref().unwrap_or_default();
        let mut line = Line::from(vec![
            Span::raw(format!(" {}", padded(row.spec.display, display_width))),
            Span::styled(padded(row.spec.binary, binary_width), DIM),
            Span::styled(text::clip(reason, reason_width), DIM),
        ]);
        if row.blocked.is_some() {
            line = line.style(DIM);
        }
        if index == modal.selected {
            line = line.style(SELECTED_ROW);
        }
        lines.push(line);
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        " the ui steps aside while the harness runs and returns when it exits",
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

/// Bindings the help shows in one cell: side by side under a verb they
/// share, or with `/` between the keys and ` / ` between the verbs.
const JOINED: [&[&str]; 2] = [&["j/k", "↑/↓"], &["y", "Y"]];
/// Cells between a key and its verb, and between a verb and the next key.
const KEY_GAP: usize = 3;
const VERB_GAP: usize = 2;
/// The one idea behind the keys, shown under them when the card is wide
/// enough for the whole sentence.
const HELP_NOTE: &str = "  every key is a hedos subcommand: p is pull, x is rm, w is warm.";

/// One line of a help column.
enum HelpCell {
    Header(&'static str),
    Blank,
    Row { key: String, verb: String },
}

/// A help cell as `(keys, gloss)`: bindings that share a gloss are listed
/// side by side under it, the rest are joined with `/` and their glosses
/// with ` / `.
fn help_cell(keys: &[&str]) -> (String, String) {
    let bindings: Vec<&keymap::Binding> =
        keys.iter().filter_map(|key| keymap::binding(key)).collect();
    let mut glosses: Vec<&str> = Vec::new();
    for binding in &bindings {
        if !glosses.contains(&binding.gloss()) {
            glosses.push(binding.gloss());
        }
    }
    let keys: Vec<&str> = bindings.iter().map(|binding| binding.key).collect();
    let separator = if glosses.len() == 1 { " " } else { "/" };
    (keys.join(separator), glosses.join(" / "))
}

/// A group's heading and its bindings in the order they are declared, the
/// joined ones as one cell.
fn group_cells(group: keymap::Group) -> Vec<HelpCell> {
    let mut cells = vec![HelpCell::Header(group.label())];
    let mut joined_already: Vec<&str> = Vec::new();
    for binding in keymap::BINDINGS
        .iter()
        .filter(|binding| binding.group == group)
    {
        if joined_already.contains(&binding.key) {
            continue;
        }
        let keys = JOINED
            .iter()
            .find(|set| set[0] == binding.key)
            .copied()
            .unwrap_or(std::slice::from_ref(&binding.key));
        joined_already.extend(&keys[1..]);
        let (key, verb) = help_cell(keys);
        cells.push(HelpCell::Row { key, verb });
    }
    cells
}

/// `top` over `bottom` in one column, a blank between them.
fn stacked(top: keymap::Group, bottom: keymap::Group) -> Vec<HelpCell> {
    let mut cells = group_cells(top);
    cells.push(HelpCell::Blank);
    cells.extend(group_cells(bottom));
    cells
}

/// The help in three columns, the screen keys under the move keys.
fn three_columns() -> Vec<Vec<HelpCell>> {
    use keymap::Group;
    vec![
        stacked(Group::Move, Group::Screen),
        group_cells(Group::Model),
        group_cells(Group::Shelf),
    ]
}

/// The help in two columns, the shelf keys under the model keys as well.
fn two_columns() -> Vec<Vec<HelpCell>> {
    use keymap::Group;
    vec![
        stacked(Group::Move, Group::Screen),
        stacked(Group::Model, Group::Shelf),
    ]
}

/// Terminal columns from which the help lays its groups out in three
/// columns: the three-column table, the border, and a margin either side.
/// Narrower, it folds to two.
fn wide_help_from() -> u16 {
    table_width(&three_columns()) as u16 + BORDER_COLUMNS + 2 * MARGIN
}

/// The help's columns at a terminal `width` cells wide.
fn help_columns(width: u16) -> Vec<Vec<HelpCell>> {
    if width >= wide_help_from() {
        three_columns()
    } else {
        two_columns()
    }
}

/// The help card's width at a terminal `width` cells wide: the fixed width
/// with three columns, and with two, the table with a cell of air on its
/// right and the border.
fn help_width(width: u16) -> u16 {
    if width >= wide_help_from() {
        HELP_WIDTH
    } else {
        table_width(&two_columns()) as u16 + BORDER_COLUMNS + 2
    }
}

/// Cells inside the help card's border at a terminal `width` cells wide.
fn help_inner(width: u16) -> usize {
    modal_width(help_width(width), width).saturating_sub(BORDER_COLUMNS) as usize
}

/// A column's key and verb widths with their gaps: the widest key plus
/// `KEY_GAP`, the widest verb plus `VERB_GAP`.
fn column_widths(column: &[HelpCell]) -> (usize, usize) {
    let widest = |pick: fn(&String, &String) -> usize| {
        column
            .iter()
            .filter_map(|cell| match cell {
                HelpCell::Row { key, verb } => Some(pick(key, verb)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    (
        widest(|key, _| key.width()) + KEY_GAP,
        widest(|_, verb| verb.width()) + VERB_GAP,
    )
}

/// The key table: `columns` side by side, each as wide as its widest key
/// and verb, the last one unpadded and every row trimmed on the right.
fn table_lines(columns: &[Vec<HelpCell>]) -> Vec<Line<'static>> {
    let widths: Vec<(usize, usize)> = columns.iter().map(|column| column_widths(column)).collect();
    let rows = columns.iter().map(Vec::len).max().unwrap_or(0);
    let mut lines = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans = vec![Span::raw("  ")];
        for (index, (column, (key_width, verb_width))) in columns.iter().zip(&widths).enumerate() {
            let last = index + 1 == columns.len();
            let cell_width = key_width + verb_width;
            let fill = |text: &str, width: usize| {
                if last {
                    text.to_owned()
                } else {
                    padded(text, width)
                }
            };
            match column.get(row) {
                Some(HelpCell::Header(name)) => {
                    spans.push(Span::styled(fill(name, cell_width), EYEBROW));
                }
                Some(HelpCell::Row { key, verb }) => {
                    spans.push(Span::styled(padded(key, *key_width), DIM));
                    spans.push(Span::raw(fill(verb, *verb_width)));
                }
                Some(HelpCell::Blank) | None => spans.push(Span::raw(fill("", cell_width))),
            }
        }
        while spans
            .last()
            .is_some_and(|span| span.content.trim().is_empty())
        {
            spans.pop();
        }
        if let Some(last) = spans.last_mut() {
            last.content = last.content.trim_end().to_owned().into();
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Cells the widest row of the key table for `columns` takes.
fn table_width(columns: &[Vec<HelpCell>]) -> usize {
    table_lines(columns)
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
}

/// The key table and its closer, then the one idea behind the keys when
/// the card is wide enough for it; the closer comes first so a short
/// terminal clips the note, not the way out.
fn help(width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    lines.extend(table_lines(&help_columns(width)));
    lines.push(Line::default());
    lines.push(keys(&[("esc", "close")]));
    if help_inner(width) >= HELP_NOTE.width() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(HELP_NOTE, DIM)));
    }
    lines
}

/// The help modal's rows at a terminal `width` cells wide: its lines and
/// the border.
fn help_height(width: u16) -> u16 {
    help(width).len() as u16 + BORDER_ROWS
}

/// The width of the label column.
fn label_column() -> usize {
    label_width(&LABELS, 2)
}

/// The width of a pull row's provider column: the widest provider id among
/// `matches`.
fn provider_width(matches: &[PullMatch]) -> usize {
    let ids: Vec<&str> = matches
        .iter()
        .map(|candidate| candidate.provider.as_str())
        .collect();
    label_width(&ids, 0)
}

/// What removing the model does, in the store's own terms.
fn remove(preview: &ModelDeletionPreview, facts: &Facts, inner: Rect) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, label_column());
    let value_width = (inner.width as usize).saturating_sub(label_column() + 2);
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
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let shown = text::home_relative(path, home.as_deref());
        lines.push(row("path", text::elide_middle(&shown, value_width)));
    }
    lines.push(Line::default());
    lines.push(Line::from(format!(" {what}")));
    lines.push(Line::from(styled_field(
        "after",
        format!(
            "{} on disk",
            text::bytes((facts.disk_bytes() - preview.bytes_estimate).max(0))
        ),
        label_column(),
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
    let row = |label, value: String| field_line(label, value, label_column());
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
        Line::from(edited(&modal.input, MARK, inner.width as usize)),
        Line::default(),
    ];
    let listing_rows = modal.rows();
    let provider_width = provider_width(&modal.matches);
    let visible = (inner.height as usize).saturating_sub(LISTING_CHROME_ROWS);
    let selected_at = listing_rows
        .iter()
        .position(|entry| matches!(entry, ListingRow::Match(index) if *index == modal.selected))
        .unwrap_or(0);
    let first = selected_at.saturating_sub(visible.saturating_sub(1));
    for entry in listing_rows.iter().skip(first).take(visible) {
        match entry {
            ListingRow::Eyebrow(category) => lines.push(Line::from(Span::styled(
                format!(" {}", category.as_str().to_uppercase()),
                EYEBROW,
            ))),
            ListingRow::Match(index) => {
                let candidate = &modal.matches[*index];
                let mut line = row(candidate, memory_bytes, provider_width, inner.width);
                if *index == modal.selected {
                    line = line.style(SELECTED_ROW);
                }
                lines.push(line);
            }
        }
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
/// elided so the columns hold.
fn row(
    candidate: &PullMatch,
    memory_bytes: u64,
    provider_width: usize,
    width: u16,
) -> Line<'static> {
    let verdict = candidate.fit(memory_bytes);
    let (size, note) = match (candidate.pulling, candidate.bytes) {
        (true, bytes) => (
            bytes.map(text::bytes).unwrap_or_default(),
            "downloading".to_owned(),
        ),
        (false, Some(bytes)) => (text::bytes(bytes), verdict_label(verdict).to_owned()),
        (false, None) => (String::new(), candidate.note.clone()),
    };
    let tail = format!(
        "{}  {}",
        right_aligned(&size, SIZE_WIDTH),
        padded(&text::clip(&note, NOTE_WIDTH), NOTE_WIDTH)
    );
    let head_width = (width as usize).saturating_sub(tail.width() + provider_width + 3);
    let reference = text::clip(&candidate.reference, head_width);
    let style = if candidate.pulling || verdict == Some(FitVerdict::TooLarge) {
        DIM
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(
            format!(" {} ", padded(candidate.provider.as_str(), provider_width)),
            DIM,
        ),
        Span::raw(format!("{} ", padded(&reference, head_width))),
        Span::styled(tail, style),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::records::SourceKind;

    use crate::tui::facts::Facts;
    use crate::tui::testing::{plan, record_with};
    use crate::tui::ui::leading_label;

    #[test]
    fn every_label_is_listed() {
        let preview = ModelDeletionPreview {
            model_id: "m".to_owned(),
            name: "m".to_owned(),
            kind: SourceKind::ollama(),
            paths: vec!["/p".to_owned()],
            bytes_estimate: 0,
            via_daemon: false,
            missing: false,
        };
        let app = App::new(Vec::new(), Facts::default());
        let inner = Rect::new(0, 0, 80, 9);
        let mut seen = Vec::new();
        for line in remove(&preview, &Facts::default(), inner)
            .iter()
            .chain(&super::preview(&plan("gemma3"), &app))
        {
            let is_label = line.spans.first().is_some_and(|span| {
                span.style == DIM && span.content.width() == label_column() + 1
            });
            if !is_label {
                continue;
            }
            let label = leading_label(line, label_column());
            assert!(LABELS.contains(&label.as_str()), "{label} is not listed");
            seen.push(label);
        }
        for label in LABELS {
            assert!(
                seen.iter().any(|seen| seen == label),
                "{label} never appears"
            );
        }
    }

    use crate::tui::testing::line_text as text;

    /// The key cell of every row in `columns`, in reading order.
    fn shown_keys(columns: &[Vec<HelpCell>]) -> Vec<String> {
        columns
            .iter()
            .flatten()
            .filter_map(|cell| match cell {
                HelpCell::Row { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    /// Every binding's key as the help cells it: joined bindings as their
    /// one cell, the rest as themselves.
    fn bound_keys() -> Vec<String> {
        let joined = |key: &str| JOINED.iter().find(|set| set.contains(&key));
        keymap::BINDINGS
            .iter()
            .filter_map(|binding| match joined(binding.key) {
                Some(set) if set[0] != binding.key => None,
                Some(set) => Some(help_cell(set).0),
                None => Some(binding.key.to_owned()),
            })
            .collect()
    }

    /// The table rows of the help at `width`: the lines after the leading
    /// blank, as many as the tallest column.
    fn table_rows(width: u16) -> Vec<String> {
        let rows = help_columns(width).iter().map(Vec::len).max().unwrap_or(0);
        help(width).iter().skip(1).take(rows).map(text).collect()
    }

    #[test]
    fn every_binding_is_in_the_help() {
        for set in JOINED {
            for key in set {
                assert!(keymap::binding(key).is_some(), "{key} is not bound");
            }
        }
        let wide = wide_help_from();
        for width in [wide, wide - 1] {
            let shown = shown_keys(&help_columns(width));
            let mut bound = bound_keys();
            let mut listed = shown.clone();
            bound.sort_unstable();
            listed.sort_unstable();
            assert_eq!(listed, bound, "the help at {width} and the keymap differ");
            let rendered = table_rows(width);
            for key in shown {
                assert!(
                    rendered.iter().any(|line| line.contains(&key)),
                    "{key} is not rendered at {width}"
                );
            }
        }
    }

    #[test]
    fn the_help_reads_as_planned() {
        assert_eq!(
            help_cell(&["j/k", "↑/↓"]),
            ("j/k ↑/↓".to_owned(), "move".to_owned())
        );
        assert_eq!(
            help_cell(&["y", "Y"]),
            ("y/Y".to_owned(), "copy path / id".to_owned())
        );
        let wide = wide_help_from();
        let rows = table_rows(wide);
        assert!(
            rows.iter().any(|line| line.contains("MOVE")
                && line.contains("MODEL")
                && line.contains("SHELF"))
        );
        assert!(rows.iter().any(|line| line.starts_with("  SCREEN")));
        assert!(rows.iter().any(|line| line.contains("launch a harness")));
        assert!(rows.iter().any(|line| line.contains("esc       collapse")));
        assert!(rows.iter().any(|line| line.contains("chat in terminal")));
        assert!(
            rows.iter()
                .any(|line| line.contains("y/Y   copy path / id"))
        );
        for row in &rows {
            assert_eq!(row, row.trim_end(), "{row:?} carries trailing air");
        }
        let rendered: Vec<String> = help(wide).iter().map(text).collect();
        let closer = rendered
            .iter()
            .position(|line| line.trim_end() == " esc close");
        assert_eq!(
            closer,
            Some(rows.len() + 2),
            "the closer is not under the table"
        );
        assert_eq!(rendered.last().map(String::as_str), Some(HELP_NOTE));
        assert_eq!(HELP_NOTE.width(), help_inner(wide));
    }

    #[test]
    fn help_columns_never_run_together() {
        let wide = wide_help_from();
        for width in [wide, wide - 1] {
            let columns = help_columns(width);
            for (row, shown) in table_rows(width).iter().enumerate() {
                let mut offset = 2;
                for column in &columns {
                    let (key_width, verb_width) = column_widths(column);
                    if let Some(HelpCell::Row { key, verb }) = column.get(row) {
                        let before: String = shown.chars().take(offset).collect();
                        let cell: String = shown.chars().skip(offset).collect();
                        assert!(
                            before.ends_with("  "),
                            "{key} has no gutter before it in {shown:?}"
                        );
                        assert!(
                            cell.starts_with(key.as_str()),
                            "{key} is not at {offset} in {shown:?}"
                        );
                        let after_key: String = shown.chars().skip(offset + key.width()).collect();
                        assert!(
                            after_key.starts_with("  "),
                            "{key} runs into {verb} in {shown:?}"
                        );
                    }
                    offset += key_width + verb_width;
                }
            }
        }
    }

    /// The card's inner width: what its border leaves of `width`.
    fn inner(width: u16) -> usize {
        width.saturating_sub(BORDER_COLUMNS) as usize
    }

    /// The help's rows at `width`: a blank, the table, a blank, the closer,
    /// and with the note a blank and the note, inside the border.
    fn expected_help_height(width: u16, with_note: bool) -> u16 {
        let rows = help_columns(width).iter().map(Vec::len).max().unwrap_or(0) as u16;
        let note = if with_note { 2 } else { 0 };
        1 + rows + 1 + 1 + note + BORDER_ROWS
    }

    #[test]
    fn the_help_folds_to_two_columns_on_a_narrow_terminal() {
        let wide = wide_help_from();
        assert_eq!(wide, 71);
        assert_eq!(help_columns(wide).len(), 3);
        assert_eq!(help_columns(wide - 1).len(), 2);
        assert_eq!(help_width(wide), HELP_WIDTH);
        assert_eq!(help_width(wide - 1), 52);
        let narrow = help(wide - 1);
        assert!(
            narrow
                .iter()
                .all(|line| line.width() <= inner(help_width(wide - 1)))
        );
        let rendered: Vec<String> = narrow.iter().map(text).collect();
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("MOVE") && line.contains("MODEL"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("SCREEN") && !line.contains("MOVE"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.starts_with("  q ") && line.contains("SHELF"))
        );
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("hedos subcommand"))
        );
        assert_eq!(
            rendered.last().map(|line| line.trim_end()),
            Some(" esc close")
        );
        assert!(help_height(wide - 1) > help_height(wide));
        assert_eq!(help_height(wide), expected_help_height(wide, true));
        assert_eq!(help_height(wide), 17);
        assert_eq!(help_height(wide - 1), expected_help_height(wide - 1, false));
        assert_eq!(help_height(wide - 1), 22);
    }

    #[test]
    fn every_modal_fits_its_width() {
        let fits = |lines: &[Line], width: usize, modal: &str| {
            for line in lines {
                assert!(
                    line.width() <= width,
                    "{:?} is {} cells, wider than the {modal}",
                    text(line),
                    line.width()
                );
            }
        };
        let wide = wide_help_from();
        for width in [wide, wide + 1, 120] {
            fits(&help(width), help_inner(width), "help");
        }
        for width in [wide - 1, 56] {
            fits(&help(width), help_inner(width), "narrow help");
            assert_eq!(help_inner(width), inner(help_width(width)));
        }
        assert!(help_inner(55) < inner(help_width(55)));
        let app = App::new(Vec::new(), Facts::default());
        fits(
            &super::preview(&plan("gemma3"), &app),
            inner(PULL_WIDTH),
            "pull",
        );

        let launch_inner = Rect::new(0, 0, inner(LAUNCH_WIDTH) as u16, LAUNCH_HEIGHT);
        let launch = LaunchModal::open_with(&record_with("m", Vec::new()), |_| None);
        assert!(launch.rows.iter().all(|row| row.blocked.is_some()));
        let launch = super::launch(&launch, launch_inner);
        fits(&launch, inner(LAUNCH_WIDTH), "launch");
        assert!(
            launch
                .iter()
                .map(text)
                .any(|line| line.contains("not installed"))
        );
        assert!(
            launch
                .iter()
                .map(text)
                .any(|line| line.contains("harness runs"))
        );

        let remove_inner = Rect::new(0, 0, inner(REMOVE_WIDTH) as u16, REMOVE_HEIGHT);
        let preview = ModelDeletionPreview {
            model_id: "m".to_owned(),
            name: "m".to_owned(),
            kind: SourceKind::ollama(),
            paths: vec![format!("/var/lib/ollama/models/blobs/{}", "a".repeat(120))],
            bytes_estimate: 0,
            via_daemon: false,
            missing: false,
        };
        let remove = remove(&preview, &Facts::default(), remove_inner);
        fits(&remove, inner(REMOVE_WIDTH), "remove");
        assert!(remove.iter().map(text).any(|line| line.contains('…')));

        assert_eq!(modal_width(84, 120), 84);
        assert_eq!(modal_width(84, 80), 80 - 2 * MARGIN);
        assert_eq!(modal_width(72, 3), 0);
    }

    #[test]
    fn the_remove_path_is_elided_to_the_modal() {
        let root = "/var/lib/ollama/models/blobs/";
        let path = format!("{root}{}", "a".repeat(120 - root.len()));
        assert_eq!(path.len(), 120);
        let preview = ModelDeletionPreview {
            model_id: "m".to_owned(),
            name: "m".to_owned(),
            kind: SourceKind::ollama(),
            paths: vec![path],
            bytes_estimate: 0,
            via_daemon: false,
            missing: false,
        };
        let inner = Rect::new(0, 0, 80, 9);
        let lines = remove(&preview, &Facts::default(), inner);
        let path_line = lines
            .iter()
            .find(|line| text(line).contains("path"))
            .map(text)
            .unwrap_or_default();
        assert!(path_line.contains('…'));
        assert!(path_line.starts_with(" path"));
        assert!(path_line.ends_with("aaaa"));
        assert!(Line::from(path_line.as_str()).width() <= 80);
        let after = lines
            .iter()
            .find(|line| text(line).contains("after"))
            .map(text)
            .unwrap_or_default();
        assert!(after.starts_with(" after") && after.ends_with("on disk"));
        assert!(!after.contains(':'));
    }
}
