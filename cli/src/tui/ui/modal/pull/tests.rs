use super::*;

use crate::tui::testing::{facts_with_memory, plan, text, texts};
use crate::tui::ui::SELECTED_MARK;

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
        // The selected row carries the gutter mark where its space was.
        let shown = text(line);
        let shown = shown
            .strip_prefix(SELECTED_MARK)
            .or_else(|| shown.strip_prefix(' '))
            .unwrap_or_default();
        shown.starts_with("ollama") || shown.starts_with("huggingface")
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
        .find(|offer| offer.bytes.is_some() && offer.note.contains(' '))
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
    let size_only = (head + SIZE_WIDTH + 2 + SIZE_MIN_ROOM) as u16;
    Anchors {
        noted: (head + SIZE_WIDTH + 2 + NOTE_MIN_ROOM) as u16,
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
        .find(|offer| offer.provider.as_str() == "ollama")
        .map(|offer| offer.note.split(' ').next().unwrap_or_default().to_owned())
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
