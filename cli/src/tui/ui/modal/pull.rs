//! The pull card through its stages: the listing of what can be pulled, the
//! spinner while a plan is resolved, the plan's preview, and a note to read
//! before going back.

use kernel::install::plan::InstallPlan;
use kernel::profiles::FitVerdict;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{BORDER_ROWS, label_column};
use crate::support::shelf_table::verdict_label;
use crate::tui::app::App;
use crate::tui::pull::{
    CATEGORIES, ListingRow, MAX_MATCHES, PullMatch, PullModal, Stage, footprint_mb,
};
use crate::tui::text;
use crate::tui::ui::detail::fit_summary;
use crate::tui::ui::{
    ACCENT, CAUTION, DIM, EYEBROW, SELECTED_ROW, edited, field_line, keys, padded, right_aligned,
    spinner, styled_field, value_width, widest,
};
use crate::tui::wrap;

/// The pull modal's width: room for a listing row with its reference
/// column at [`MAX_REFERENCE_WIDTH`], its size, and most catalog blurbs
/// whole beside them; narrower terminals clamp it and the note elides.
pub(super) const PULL_WIDTH: u16 = 108;
/// Rows of the listing that are not matches: the input, a blank, the keys.
const LISTING_CHROME_ROWS: usize = 3;
/// Rows of the note stage that are not the note: a blank above, a blank
/// and the keys below.
const NOTE_CHROME_ROWS: usize = 3;
/// The pull modal: the border, the input and a blank, every match with an
/// eyebrow per category and a blank before every eyebrow but the first,
/// and the keys.
pub(super) const PULL_HEIGHT: u16 =
    (MAX_MATCHES + CATEGORIES.len() + (CATEGORIES.len() - 1) + LISTING_CHROME_ROWS) as u16
        + BORDER_ROWS;
/// Cells the size column of a pull row holds: `999.9 GB` at the widest.
const SIZE_WIDTH: usize = 8;
/// The widest a pull row's reference column grows; longer references are
/// clipped so the note keeps its room.
const MAX_REFERENCE_WIDTH: usize = 32;
/// The narrowest note worth printing; under it the row ends at the size.
const NOTE_FLOOR: usize = 12;
/// Under this much room for the note, the size goes too and the row is the
/// reference alone.
const SIZE_FLOOR: usize = 8;
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
            fit_summary(plan.total_bytes.map(footprint_mb), app.facts.memory_bytes),
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
/// `matches`.
fn provider_width(matches: &[PullMatch]) -> usize {
    let ids: Vec<&str> = matches
        .iter()
        .map(|candidate| candidate.provider.as_str())
        .collect();
    widest(&ids)
}

/// The width of a pull row's reference column: the widest reference among
/// `matches`, up to [`MAX_REFERENCE_WIDTH`].
fn reference_width(matches: &[PullMatch]) -> usize {
    let references: Vec<&str> = matches
        .iter()
        .map(|candidate| candidate.reference.as_str())
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
    let visible = (inner.height as usize)
        .saturating_sub(LISTING_CHROME_ROWS + usize::from(modal.direct_installed.is_some()));
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
            ListingRow::Blank => lines.push(Line::default()),
            ListingRow::Match(index) => {
                let candidate = &modal.matches[*index];
                let mut line = row(candidate, memory_bytes, widths, inner.width);
                if *index == modal.selected {
                    line = line.style(SELECTED_ROW);
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
    candidate: &PullMatch,
    memory_bytes: u64,
    (provider_width, reference_width): (usize, usize),
    width: u16,
) -> Line<'static> {
    let verdict = candidate.fit(memory_bytes);
    let note = if candidate.pulling {
        "downloading".to_owned()
    } else if matches!(verdict, Some(FitVerdict::TightFit | FitVerdict::TooLarge)) {
        verdict_label(verdict).to_owned()
    } else {
        candidate.note.clone()
    };
    let note_style = if !candidate.pulling && verdict == Some(FitVerdict::TightFit) {
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
            format!(" {} ", padded(candidate.provider.as_str(), provider_width)),
            DIM,
        ),
        Span::raw(format!(
            "{} ",
            padded(
                &text::clip(&candidate.reference, reference_width),
                reference_width
            )
        )),
    ];
    if note_width >= SIZE_FLOOR {
        let size = candidate.bytes.map(text::bytes).unwrap_or_default();
        spans.push(Span::raw(right_aligned(&size, SIZE_WIDTH)));
    }
    if note_width >= NOTE_FLOOR {
        spans.push(Span::styled(
            format!("  {}", text::clip(&note, note_width)),
            note_style,
        ));
    }
    let line = Line::from(spans);
    if candidate.pulling || verdict == Some(FitVerdict::TooLarge) {
        line.style(DIM)
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::{facts_with_memory, plan, text, texts};

    const MEMORY: u64 = 64 << 30;

    #[test]
    fn the_card_budgets_every_row_of_a_grouped_listing() {
        let modal = PullModal::open(&[], MEMORY, &[]);
        assert!(modal.rows().len() + LISTING_CHROME_ROWS <= (PULL_HEIGHT - BORDER_ROWS) as usize);
        let blanks = modal
            .rows()
            .iter()
            .filter(|row| **row == ListingRow::Blank)
            .count();
        assert_eq!(blanks, CATEGORIES.len() - 1);
    }

    #[test]
    fn the_preview_elides_its_destination() {
        let app = App::new(Vec::new(), facts_with_memory(64));
        let mut long = plan("gemma3");
        long.destination = format!("/var/lib/ollama/models/blobs/{}", "a".repeat(120));
        long.total_bytes = Some(4 << 30);
        long.remaining_bytes = Some(1 << 30);
        let inner = Rect::new(0, 0, 80, 12);
        let lines = texts(&preview(&long, &app, inner));
        let to = lines
            .iter()
            .find(|line| line.starts_with(" to"))
            .cloned()
            .unwrap_or_default();
        assert!(to.contains('…') && to.ends_with("aaaa"), "{to:?}");
        assert!(Line::from(to.as_str()).width() <= 80);
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(" size") && line.ends_with("4 GB"))
        );
        assert!(
            lines.iter().any(|line| line.starts_with(" fit")
                && line.contains("fits · needs ")
                && line.ends_with(" of 64 GiB")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(" download") && line.ends_with("1 GB more"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(" after") && line.ends_with("1 GB on disk"))
        );
        let mut fresh = long.clone();
        fresh.remaining_bytes = Some(4 << 30);
        let fresh = texts(&preview(&fresh, &app, inner));
        assert!(
            fresh
                .iter()
                .any(|line| line.starts_with(" download") && line.ends_with("  4 GB"))
        );
        let mut done = long;
        done.remaining_bytes = Some(0);
        let done = texts(&preview(&done, &app, inner));
        assert!(done.iter().any(|line| line.ends_with("already on disk")));
        let unknown = texts(&preview(&plan("gemma3"), &app, inner));
        assert!(
            unknown
                .iter()
                .any(|line| line.starts_with(" fit") && line.ends_with("unknown footprint"))
        );
        assert!(!unknown.iter().any(|line| line.trim() == "gemma3"));
    }

    /// The listing rows of a fresh modal at `width` cells inside the card.
    fn listing_rows(width: u16) -> Vec<Line<'static>> {
        let modal = PullModal::open(&[], MEMORY, &[]);
        listing(
            &modal,
            MEMORY,
            Rect::new(0, 0, width, PULL_HEIGHT - BORDER_ROWS),
        )
        .into_iter()
        .filter(|line| {
            let shown = text(line);
            shown.starts_with(" ollama") || shown.starts_with(" huggingface")
        })
        .collect()
    }

    #[test]
    fn a_listing_row_shows_the_blurb_when_it_fits() {
        let modal = PullModal::open(&[], MEMORY, &[]);
        let widths = (
            provider_width(&modal.matches),
            reference_width(&modal.matches),
        );
        assert!(widths.1 <= MAX_REFERENCE_WIDTH);
        let gemma = modal
            .matches
            .iter()
            .find(|candidate| candidate.bytes.is_some() && candidate.note.contains(' '))
            .expect("a catalog entry with a blurb");
        let line = text(&row(gemma, MEMORY, widths, 120));
        assert!(line.contains(&gemma.reference), "{line:?}");
        assert!(line.contains(&gemma.note), "{line:?}");
        assert!(!line.contains("  fits"), "{line:?}");
        let mut tight = gemma.clone();
        tight.bytes = Some(44 << 30);
        let tight_line = row(&tight, MEMORY, widths, 120);
        assert!(
            text(&tight_line).ends_with("tight"),
            "{:?}",
            text(&tight_line)
        );
        assert!(
            tight_line
                .spans
                .last()
                .is_some_and(|span| span.style == CAUTION)
        );
        let mut big = gemma.clone();
        big.bytes = Some(200 << 30);
        let big_line = row(&big, MEMORY, widths, 120);
        assert!(text(&big_line).ends_with("too big"));
        assert_eq!(big_line.style, DIM);
        let mut pulling = gemma.clone();
        pulling.pulling = true;
        let pulling_line = row(&pulling, MEMORY, widths, 120);
        assert!(text(&pulling_line).ends_with("downloading"));
        assert_eq!(pulling_line.style, DIM);
        let mut typed = gemma.clone();
        typed.bytes = None;
        typed.note = "as typed".to_owned();
        assert!(text(&row(&typed, MEMORY, widths, 120)).ends_with("as typed"));
    }

    /// The widths a listing row changes shape at, from the columns of a
    /// fresh listing: the note's floor, the size's floor, one under it, and
    /// a card too narrow for the reference column.
    struct Anchors {
        noted: u16,
        size_only: u16,
        reference_only: u16,
        clipped: u16,
    }

    fn anchors() -> Anchors {
        let modal = PullModal::open(&[], MEMORY, &[]);
        let head = 1 + provider_width(&modal.matches) + 1 + reference_width(&modal.matches) + 1;
        let size_only = (head + SIZE_WIDTH + 2 + SIZE_FLOOR) as u16;
        Anchors {
            noted: (head + SIZE_WIDTH + 2 + NOTE_FLOOR) as u16,
            size_only,
            reference_only: size_only - 1,
            clipped: head as u16 - 3,
        }
    }

    #[test]
    fn a_listing_row_sheds_its_note_then_its_size() {
        let at = anchors();
        for width in [at.noted, at.size_only, at.reference_only, at.clipped] {
            for line in listing_rows(width) {
                assert!(
                    line.width() <= width as usize,
                    "{:?} runs past {width}",
                    text(&line)
                );
                assert!(!text(&line).trim_end().ends_with("fits"));
            }
        }
        let full = listing_rows(at.noted);
        assert!(
            full.iter().all(|line| text(line).contains("B  ")),
            "{:?}",
            text(&full[0])
        );
        let modal = PullModal::open(&[], MEMORY, &[]);
        let blurb = modal
            .matches
            .iter()
            .find(|candidate| candidate.provider.as_str() == "ollama")
            .map(|candidate| {
                candidate
                    .note
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .unwrap_or_default();
        assert!(!blurb.is_empty());
        assert!(
            full.iter()
                .any(|line| text(line).contains(&format!("GB  {blurb}")))
        );
        let unnoted = listing_rows(at.noted - 1);
        assert!(!unnoted.iter().any(|line| text(line).contains(&blurb)));
        assert!(
            unnoted
                .iter()
                .all(|line| text(line).trim_end().ends_with("B"))
        );
        let sized = listing_rows(at.size_only);
        assert!(
            sized
                .iter()
                .all(|line| text(line).trim_end().ends_with("B"))
        );
        assert!(!sized.iter().any(|line| text(line).contains(&blurb)));
        let bare = listing_rows(at.reference_only);
        assert!(
            !bare
                .iter()
                .any(|line| text(line).contains(" GB") || text(line).contains(" MB"))
        );
        assert!(
            bare.iter()
                .any(|line| text(line).contains(&modal.matches[0].reference))
        );
        let narrow = listing_rows(at.clipped);
        assert!(
            narrow
                .iter()
                .all(|line| line.width() <= at.clipped as usize)
        );
        assert!(narrow.iter().any(|line| text(line).contains('…')));
    }

    #[test]
    fn the_listing_names_a_typed_model_already_on_the_shelf() {
        use crate::tui::event::Key;
        let mut modal = PullModal::open(&[], MEMORY, &[]);
        let inner = Rect::new(0, 0, 82, PULL_HEIGHT - BORDER_ROWS);
        let blank = texts(&listing(&modal, MEMORY, inner));
        assert_eq!(blank[0].trim_end(), " › name, owner/repo or name:tag");
        assert!(
            !blank
                .iter()
                .any(|line| line.contains("already on the shelf"))
        );
        let first = modal.matches[0].reference.clone();
        let shelf = vec![crate::tui::testing::record(&first)];
        modal.refresh(&shelf, &[]);
        for c in first.chars() {
            modal.edit(Key::Char(c), 0);
        }
        assert_eq!(modal.direct_installed.as_deref(), Some(first.as_str()));
        let lines = listing(&modal, MEMORY, inner);
        assert!(lines.len() <= inner.height as usize);
        let shown = texts(&lines);
        assert!(
            shown.contains(&format!(" {first} is already on the shelf")),
            "{shown:?}"
        );
        assert_eq!(
            shown.last().map(|line| line.trim_end()),
            Some(" enter choose  ↑/↓ move  esc close")
        );
    }
}
