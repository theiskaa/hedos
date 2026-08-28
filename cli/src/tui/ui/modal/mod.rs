//! The modals (pull, remove, help, launch), drawn over a dimmed screen; the
//! chat pane, though it sits in the same slot, is drawn by `chat` instead.
//! This module owns the card: its width, its border, its title; each card's
//! body lives in a module of its own.

mod help;
mod launch;
mod pull;
mod remove;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::{ACCENT, BACKDROP, DIM, centered, label_width};
use crate::tui::app::{App, Modal};

/// Cells kept clear on either side of a modal when the terminal is narrower
/// than it wants.
const MARGIN: u16 = 2;
/// Rows a bordered box spends on its top and bottom edges.
const BORDER_ROWS: u16 = 2;
/// Columns a bordered box spends on its left and right edges.
const BORDER_COLUMNS: u16 = 2;
/// The labels of the preview and remove bodies; the column is as wide as
/// the widest, plus a gap.
const LABELS: [&str; 9] = [
    "store", "on disk", "path", "after", "from", "to", "size", "fit", "download",
];

/// Draw the open modal over `area`, if there is one.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let Some(modal) = &app.modal else {
        return;
    };
    let (width, height, title, body): (u16, u16, String, Body) = match modal {
        // The chat pane is a body of its own, not something over the shelf.
        Modal::Chat(_) => return,
        Modal::Pull(modal) => (
            pull::PULL_WIDTH,
            pull::PULL_HEIGHT,
            pull::pull_title(modal),
            Box::new(move |inner| pull::pull(modal, app, inner)),
        ),
        Modal::Remove(preview) => (
            remove::REMOVE_WIDTH,
            remove::REMOVE_HEIGHT,
            format!(" remove {} ", preview.name),
            Box::new(move |inner| remove::remove(preview, &app.facts, inner)),
        ),
        Modal::Help => {
            let help = help::HelpLayout::at(area.width);
            (
                help.width,
                help.height(),
                " help ".to_owned(),
                Box::new(move |_| help.lines()),
            )
        }
        Modal::Launch(modal) => (
            launch::LAUNCH_WIDTH,
            launch::LAUNCH_HEIGHT,
            format!(" launch on {} ", modal.record.display_name()),
            Box::new(move |inner| launch::launch(modal, inner)),
        ),
    };
    frame.buffer_mut().set_style(area, BACKDROP);
    let rect = centered(area, modal_width(width, area.width), height);
    let block = Block::bordered().border_style(DIM);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block.title(Span::styled(title, ACCENT)), rect);
    frame.render_widget(Paragraph::new(body(inner)), inner);
}

/// A card's lines, given the rect inside its border.
type Body<'a> = Box<dyn FnOnce(Rect) -> Vec<Line<'static>> + 'a>;

/// `wanted` cells, or what `available` leaves once a margin is kept on both
/// sides.
fn modal_width(wanted: u16, available: u16) -> u16 {
    wanted.min(available.saturating_sub(2 * MARGIN))
}

/// The width of the label column.
fn label_column() -> usize {
    label_width(&LABELS, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    use unicode_width::UnicodeWidthStr;

    use crate::tui::facts::Facts;
    use crate::tui::launch::LaunchModal;
    use crate::tui::pull::{PullModal, Stage};
    use crate::tui::testing::{deletion_preview, plan, record_with, text, texts};
    use crate::tui::ui::leading_label;

    #[test]
    fn every_label_is_listed() {
        let preview = deletion_preview(vec!["/p".to_owned()]);
        let app = App::new(Vec::new(), Facts::default());
        let inner = Rect::new(0, 0, 80, 9);
        let mut seen = Vec::new();
        for line in remove::remove(&preview, &Facts::default(), inner)
            .iter()
            .chain(&pull::preview(&plan("gemma3"), &app, inner))
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

    #[test]
    fn a_modal_keeps_a_margin_on_a_narrow_terminal() {
        assert_eq!(modal_width(84, 120), 84);
        assert_eq!(modal_width(84, 80), 80 - 2 * MARGIN);
        assert_eq!(modal_width(72, 3), 0);
    }

    /// The card's inner width: what its border leaves of `width`.
    fn inner(width: u16) -> usize {
        width.saturating_sub(BORDER_COLUMNS) as usize
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
        let wide = help::three_columns_from();
        for width in [wide, wide + 1, 120] {
            let help = help::HelpLayout::at(width);
            fits(&help.lines(), help.inner, "help");
        }
        // The narrow card fits whole down to its own width plus the margins.
        let narrow = help::HelpLayout::at(wide - 1);
        let snug = narrow.width + 2 * MARGIN;
        assert!(narrow.folded() && snug < wide);
        for width in [wide - 1, snug] {
            let help = help::HelpLayout::at(width);
            fits(&help.lines(), help.inner, "narrow help");
            assert_eq!(help.inner, inner(help.width));
        }
        let squeezed = help::HelpLayout::at(snug - 1);
        assert!(squeezed.inner < inner(squeezed.width));

        let app = App::new(Vec::new(), Facts::default());
        let pull_inner = Rect::new(
            0,
            0,
            inner(pull::PULL_WIDTH) as u16,
            pull::PULL_HEIGHT - BORDER_ROWS,
        );
        fits(
            &pull::preview(&plan("gemma3"), &app, pull_inner),
            inner(pull::PULL_WIDTH),
            "pull",
        );
        let mut modal = PullModal::open(&[], 64 << 30, &[]);
        fits(
            &pull::listing(&modal, 64 << 30, pull_inner),
            inner(pull::PULL_WIDTH),
            "pull listing",
        );
        modal.stage = Stage::Note("word ".repeat(40).trim_end().to_owned());
        let note = pull::pull(&modal, &app, pull_inner);
        fits(&note, inner(pull::PULL_WIDTH), "pull note");
        assert!(
            note.iter()
                .filter(|line| text(line).contains("word"))
                .count()
                > 1
        );
        assert_eq!(text(&note[note.len() - 1]).trim_end(), " esc back");

        let launch_inner = Rect::new(
            0,
            0,
            inner(launch::LAUNCH_WIDTH) as u16,
            launch::LAUNCH_HEIGHT,
        );
        let launch = LaunchModal::open_with(&record_with("m", Vec::new()), |_| None);
        assert!(launch.rows.iter().all(|row| row.blocked.is_some()));
        let launch = launch::launch(&launch, launch_inner);
        fits(&launch, inner(launch::LAUNCH_WIDTH), "launch");
        assert!(
            texts(&launch)
                .iter()
                .any(|line| line.contains("not installed"))
        );
        assert!(
            texts(&launch)
                .iter()
                .any(|line| line.contains("harness runs"))
        );

        let remove_inner = Rect::new(
            0,
            0,
            inner(remove::REMOVE_WIDTH) as u16,
            remove::REMOVE_HEIGHT,
        );
        let preview = deletion_preview(vec![format!(
            "/var/lib/ollama/models/blobs/{}",
            "a".repeat(120)
        )]);
        let remove = remove::remove(&preview, &Facts::default(), remove_inner);
        fits(&remove, inner(remove::REMOVE_WIDTH), "remove");
        assert!(texts(&remove).iter().any(|line| line.contains('…')));
    }
}
