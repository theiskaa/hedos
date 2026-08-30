//! The pull card through its stages: the listing of what can be pulled, the
//! spinner while a plan is resolved, the plan's preview, and a note to read
//! before going back.

use kernel::install::plan::InstallPlan;
use kernel::profiles::FitVerdict;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::label_column;
use crate::support::shelf_table::verdict_label;
use crate::tui::app::App;
use crate::tui::pull::{
    CATEGORIES, ListingRow, MAX_MATCHES, Offer, PullModal, Stage, footprint_mb,
};
use crate::tui::text;
use crate::tui::ui::{
    ACCENT, BORDER_ROWS, CAUTION, DIM, EYEBROW, edited, field_line, keys, padded, right_aligned,
    selected_row, spinner, styled_field, value_width, widest,
};
use crate::tui::wrap;

/// The pull card's width: room for a listing row with its reference
/// column at [`MAX_REFERENCE_WIDTH`], its size, and most catalog blurbs
/// whole beside them; narrower terminals clamp it and the note elides.
pub(super) const PULL_WIDTH: u16 = 108;
/// Rows of the listing that are not matches: the input, a blank, the keys.
const LISTING_CHROME_ROWS: usize = 3;
/// Rows of the note stage that are not the note: a blank above, a blank
/// and the keys below.
const NOTE_CHROME_ROWS: usize = 3;
/// The pull card's height: the border, the input and a blank, every match
/// with an eyebrow per category and a blank before every eyebrow but the
/// first, and the keys.
pub(super) const PULL_HEIGHT: u16 =
    (MAX_MATCHES + CATEGORIES.len() + (CATEGORIES.len() - 1) + LISTING_CHROME_ROWS) as u16
        + BORDER_ROWS;
/// Cells the size column of a pull row holds: `999.9 GB` at the widest.
const SIZE_WIDTH: usize = 8;
/// The widest a pull row's reference column grows; longer references are
/// clipped so the note keeps its room.
const MAX_REFERENCE_WIDTH: usize = 32;
/// The least room, in cells after the size column, a note is printed in;
/// under it the row ends at the size.
const NOTE_MIN_ROOM: usize = 12;
/// The least room, measured the same way, the size is printed with; under
/// it the size goes too and the row is the reference alone.
const SIZE_MIN_ROOM: usize = 8;
/// The prompt marker of the pull query.
const MARK: &str = " › ";
/// What the pull query accepts, shown while it is blank.
const PULL_PLACEHOLDER: &str = "name, owner/repo or name:tag";

/// ` pull `, or ` pull qwen3:8b ` once a model is being resolved or previewed.
pub(super) fn pull_title(modal: &PullModal) -> String {
    match &modal.stage {
        Stage::Listing | Stage::Note(_) => " pull ".to_owned(),
        Stage::Planning(reference) => format!(" pull {reference} "),
        Stage::Preview(plan) => format!(" pull {} ", plan.display_name),
    }
}

/// The pull modal's body for its current stage.
pub(super) fn pull(modal: &PullModal, app: &App, inner: Rect) -> Vec<Line<'static>> {
    match &modal.stage {
        Stage::Listing => listing(modal, app.facts.memory_bytes, inner),
        Stage::Planning(reference) => vec![
            Line::default(),
            Line::from(vec![
                Span::styled(format!(" {}", spinner(app.ticks())), ACCENT),
                Span::styled(format!(" resolving {reference}"), DIM),
            ]),
            Line::default(),
            keys(&[("esc", "back")]),
        ],
        Stage::Preview(plan) => preview(plan, app, inner),
        Stage::Note(note) => {
            let room = (inner.height as usize).saturating_sub(NOTE_CHROME_ROWS);
            let mut lines = vec![Line::default()];
            lines.extend(
                wrap::wrap(note, (inner.width as usize).saturating_sub(2))
                    .into_iter()
                    .take(room)
                    .map(|piece| Line::from(format!(" {piece}"))),
            );
            lines.push(Line::default());
            lines.push(keys(&[("esc", "back")]));
            lines
        }
    }
}

/// The plan as labelled rows: where from and to, how big, how it fits, what
/// the download comes to, and what the disk holds after.
pub(super) fn preview(plan: &InstallPlan, app: &App, inner: Rect) -> Vec<Line<'static>> {
    let row = |label, value: String| field_line(label, value, label_column());
    let destination = text::at_home(&plan.destination);
    let download = match (plan.remaining_bytes, plan.total_bytes) {
        (Some(0), _) => "already on disk".to_owned(),
        (Some(remaining), Some(total)) if remaining < total => {
            format!("{} more", text::bytes(remaining))
        }
        (Some(remaining), _) => text::bytes(remaining),
        (None, Some(total)) => text::bytes(total),
        (None, None) => "unknown".to_owned(),
    };
    vec![
        Line::default(),
        row(
            "from",
            format!("{} · {}", plan.provider.as_str(), plan.reference),
        ),
        row(
            "to",
            text::elide_middle(
                &destination,
                value_width(inner.width as usize, label_column()),
            ),
        ),
        row(
            "size",
            plan.total_bytes
                .map_or_else(|| "unknown".to_owned(), text::bytes),
        ),
        row(
            "fit",
            text::fit_summary(plan.total_bytes.map(footprint_mb), app.facts.memory_bytes),
        ),
        row("download", download),
        Line::from(styled_field(
            "after",
            format!(
                "{} on disk",
                text::bytes(app.facts.disk_bytes() + plan.remaining_bytes.unwrap_or(0))
            ),
            label_column(),
            DIM,
        )),
        Line::default(),
        keys(&[("enter", "pull"), ("esc", "back")]),
    ]
}

/// The width of a pull row's provider column: the widest provider id among
/// `offers`.
fn provider_width(offers: &[Offer]) -> usize {
    let ids: Vec<&str> = offers.iter().map(|offer| offer.provider.as_str()).collect();
    widest(&ids)
}

/// The width of a pull row's reference column: the widest reference among
/// `offers`, up to [`MAX_REFERENCE_WIDTH`].
fn reference_width(offers: &[Offer]) -> usize {
    let references: Vec<&str> = offers
        .iter()
        .map(|offer| offer.reference.as_str())
        .collect();
    widest(&references).min(MAX_REFERENCE_WIDTH)
}

/// The listing: the query, the matches under their eyebrows scrolled so
/// the selected one shows, a note when the typed model is already on the
/// shelf, and the keys on the last row.
pub(super) fn listing(modal: &PullModal, memory_bytes: u64, inner: Rect) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(edited(
            &modal.input,
            MARK,
            inner.width as usize,
            PULL_PLACEHOLDER,
        )),
        Line::default(),
    ];
    let listing_rows = modal.rows();
    let widths = (
        provider_width(&modal.matches),
        reference_width(&modal.matches),
    );
    let room = (inner.height as usize)
        .saturating_sub(LISTING_CHROME_ROWS + usize::from(modal.direct_installed.is_some()));
    let selected_at = listing_rows
        .iter()
        .position(|entry| matches!(entry, ListingRow::Match(index) if *index == modal.selected))
        .unwrap_or(0);
    let first = selected_at.saturating_sub(room.saturating_sub(1));
    for entry in listing_rows.iter().skip(first).take(room) {
        match entry {
            ListingRow::Eyebrow(category) => lines.push(Line::from(Span::styled(
                format!(" {}", category.as_str().to_uppercase()),
                EYEBROW,
            ))),
            ListingRow::Blank => lines.push(Line::default()),
            ListingRow::Match(index) => {
                let offer = &modal.matches[*index];
                let mut line = row(offer, memory_bytes, widths, inner.width);
                if *index == modal.selected {
                    line = selected_row(line, inner.width as usize);
                }
                lines.push(line);
            }
        }
    }
    if let Some(reference) = &modal.direct_installed {
        lines.push(Line::from(Span::styled(
            format!(" {reference} is already on the shelf"),
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

/// `provider  reference  size  note`: the provider and reference columns
/// measured, the note taking what is left. The note says what matters most,
/// in this order: `downloading`, a verdict that is not `fits`, then the
/// catalog blurb, the hit's popularity, or `as typed`. A narrow row sheds
/// the note, then the size.
fn row(
    offer: &Offer,
    memory_bytes: u64,
    (provider_width, reference_width): (usize, usize),
    width: u16,
) -> Line<'static> {
    let verdict = offer.fit(memory_bytes);
    let note = if offer.pulling {
        "downloading".to_owned()
    } else if matches!(verdict, Some(FitVerdict::TightFit | FitVerdict::TooLarge)) {
        verdict_label(verdict).to_owned()
    } else {
        offer.note.clone()
    };
    let note_style = if !offer.pulling && verdict == Some(FitVerdict::TightFit) {
        CAUTION
    } else {
        Style::new()
    };
    // A card too narrow for the whole reference column clips the reference
    // itself rather than run the row past the border.
    let reference_width = reference_width.min((width as usize).saturating_sub(provider_width + 3));
    let head = 1 + provider_width + 1 + reference_width + 1;
    let note_width = (width as usize)
        .saturating_sub(head)
        .saturating_sub(SIZE_WIDTH + 2);
    let mut spans = vec![
        Span::styled(
            format!(" {} ", padded(offer.provider.as_str(), provider_width)),
            DIM,
        ),
        Span::raw(format!(
            "{} ",
            padded(
                &text::clip(&offer.reference, reference_width),
                reference_width
            )
        )),
    ];
    if note_width >= SIZE_MIN_ROOM {
        let size = offer.bytes.map(text::bytes).unwrap_or_default();
        spans.push(Span::raw(right_aligned(&size, SIZE_WIDTH)));
    }
    if note_width >= NOTE_MIN_ROOM {
        spans.push(Span::styled(
            format!("  {}", text::clip(&note, note_width)),
            note_style,
        ));
    }
    let line = Line::from(spans);
    if offer.pulling || verdict == Some(FitVerdict::TooLarge) {
        line.style(DIM)
    } else {
        line
    }
}

#[cfg(test)]
mod tests;
