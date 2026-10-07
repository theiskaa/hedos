//! The cards (pull, remove, stop, help, launch), drawn over a dimmed screen; the
//! chat pane, though it sits in the same slot, is drawn by `chat` instead.
//! This module owns the frame around a card: the backdrop, the margin that
//! clamps a card to a narrow terminal, the border, and the title. Each card
//! owns its width, its height, and its body, in a module of its own.

mod help;
mod launch;
mod remove;
mod stop;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::card::Card;
use super::{ACCENT, BACKDROP, BORDER_ROWS, GROUND, centered, label_width};
use crate::tui::app::{App, Modal};
use crate::tui::palette::{BACKDROP_GROUND, BACKDROP_INK, FAINT_COLOR, LINE_STRONG, mix};
use ratatui::style::Style;

/// Cells kept clear on either side of a card when the terminal is narrower
/// than it wants.
const MARGIN: u16 = 2;
/// The labels of the remove and stop bodies; the column is as wide
/// as the widest, plus a gap.
const LABELS: [&str; 5] = ["store", "on disk", "path", "after", "model"];

/// Draw the open card over `area`, if there is one; whether the screen
/// under it was dimmed.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App) -> bool {
    let Some(modal) = &app.modal else {
        return false;
    };
    let (width, height, title, body): (u16, u16, String, Body) = match modal {
        // The chat pane is a body of its own, not something over the shelf.
        Modal::Chat(_) => return false,
        // So is the pull screen.
        Modal::Pull(_) => return false,
        Modal::Remove(preview) => (
            remove::REMOVE_WIDTH,
            remove::REMOVE_HEIGHT,
            format!(" remove {} ", preview.name),
            Box::new(move |inner| remove::remove(preview, &app.facts, inner)),
        ),
        Modal::Stop(card) => (
            stop::STOP_WIDTH,
            stop::STOP_HEIGHT,
            " stop pull ".to_owned(),
            Box::new(move |inner| stop::stop(card, inner)),
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
    let opened = app.modal_at().unwrap_or(0);
    dim(
        frame.buffer_mut(),
        area,
        app.motion.eased(opened, BACKDROP_MS),
    );
    // The card grows from three quarters of its height; its body is cut to
    // the rows it has, so the key line at its foot arrives last.
    let grown = app.motion.eased(opened, GROW_MS);
    let shown = ((f32::from(height) * (GROW_FROM + (1.0 - GROW_FROM) * grown)).round() as u16)
        .clamp(BORDER_ROWS.min(height), height);
    let rect = centered(area, card_width(width, area.width), shown);
    let inner = Card::inner(rect);
    let full = Rect {
        height: height.saturating_sub(BORDER_ROWS),
        ..inner
    };
    frame.render_widget(Clear, rect);
    frame.buffer_mut().set_style(rect, GROUND);
    Card::new(vec![Span::styled(title.trim().to_owned(), ACCENT)])
        .border(Style::new().fg(mix(LINE_STRONG, FAINT_COLOR, grown)))
        .render(rect, frame.buffer_mut());
    frame.render_widget(Paragraph::new(body(full)), inner);
    true
}

/// How long the screen behind a card takes to fade, and the card to grow,
/// and how tall it starts.
const BACKDROP_MS: u64 = 140;
const GROW_MS: u64 = 160;
const GROW_FROM: f32 = 0.75;

/// The screen behind a card flattened toward near black by `amount`, its
/// emphasis dropped past halfway; at 1 it is [`BACKDROP`] on every cell.
fn dim(buf: &mut ratatui::buffer::Buffer, area: Rect, amount: f32) {
    if amount >= 1.0 {
        buf.set_style(area, BACKDROP);
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = mix(cell.fg, BACKDROP_INK, amount);
            cell.bg = mix(cell.bg, BACKDROP_GROUND, amount);
            if amount > 0.5 {
                cell.modifier.remove(ratatui::style::Modifier::BOLD);
            }
        }
    }
}

/// A card's lines, given the rect inside its border.
type Body<'a> = Box<dyn FnOnce(Rect) -> Vec<Line<'static>> + 'a>;

/// `wanted` cells, or what `available` leaves once a margin is kept on both
/// sides.
fn card_width(wanted: u16, available: u16) -> u16 {
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

    use kernel::install::event::InstallProgress;

    use crate::tui::facts::Facts;
    use crate::tui::launch::LaunchModal;
    use crate::tui::stop::StopCard;
    use crate::tui::testing::{deletion_preview, leading_label, record_with, text, texts};
    use crate::tui::ui::{BORDER_COLUMNS, DIM};

    #[test]
    fn every_label_is_listed() {
        let preview = deletion_preview(vec!["/p".to_owned()]);
        let inner = Rect::new(0, 0, 80, 9);
        let mut seen = Vec::new();
        let card = StopCard {
            job: "1000-gemma3".to_owned(),
            reference: "gemma3".to_owned(),
            progress: InstallProgress::default(),
        };
        for line in remove::remove(&preview, &Facts::default(), inner)
            .iter()
            .chain(&stop::stop(&card, inner))
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

    /// The rows the open card spans in a whole frame of `app`, and the
    /// frame's top-left cell.
    fn card_rows(app: &mut App) -> (std::ops::Range<u16>, ratatui::buffer::Cell) {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40))
            .expect("a test terminal");
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, app))
            .expect("a frame");
        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String { (0..120).map(|x| buffer[(x, y)].symbol()).collect() };
        let top = (0..40).find(|&y| row(y).contains("╭─ help ")).unwrap_or(0);
        // `╭─ help`: the corner three cells before the title's first letter.
        let left = (0..117)
            .find(|&x| buffer[(x, top)].symbol() == "╭" && buffer[(x + 3, top)].symbol() == "h")
            .unwrap_or(0);
        let bottom = (top + 1..40)
            .find(|&y| buffer[(left, y)].symbol() == "╰")
            .unwrap_or(top);
        (top..bottom + 1, buffer[(0, 0)].clone())
    }

    #[test]
    fn a_card_grows_in_over_a_screen_that_fades() {
        use crate::tui::event::{Event, Key};
        let full = help::HelpLayout::at(120).height();
        let mut settled = App::new(vec![record_with("m", Vec::new())], Facts::default());
        settled.reduce(Event::Key(Key::Char('?')));
        let (rows, corner) = card_rows(&mut settled);
        assert_eq!(rows.len() as u16, full, "settled, the card is whole");
        assert_eq!(corner.fg, BACKDROP.fg.expect("the backdrop's grey"));
        assert_eq!(corner.bg, BACKDROP.bg.expect("the backdrop's ground"));

        let mut live = App::new(vec![record_with("m", Vec::new())], Facts::default());
        live.motion = crate::tui::motion::Motion::from_env_value_for_tests(None);
        live.set_clock(10_000);
        live.reduce(Event::Key(Key::Char('?')));
        let (rows, corner) = card_rows(&mut live);
        let opening = (f32::from(full) * GROW_FROM).round() as u16;
        assert_eq!(
            rows.len() as u16,
            opening,
            "just opened, it is three quarters"
        );
        assert_ne!(corner.fg, BACKDROP.fg.expect("the backdrop's grey"));
        live.set_clock(10_000 + GROW_MS + BACKDROP_MS);
        assert_eq!(card_rows(&mut live).0.len() as u16, full);
    }

    #[test]
    fn a_card_keeps_a_margin_on_a_narrow_terminal() {
        assert_eq!(card_width(84, 120), 84);
        assert_eq!(card_width(84, 80), 80 - 2 * MARGIN);
        assert_eq!(card_width(72, 3), 0);
    }

    /// The card's inner width: what its border leaves of `width`.
    fn inner(width: u16) -> usize {
        width.saturating_sub(BORDER_COLUMNS) as usize
    }

    /// Every line of `lines` fits in `width` cells, or the failure names
    /// the `card`.
    fn fits(lines: &[Line], width: usize, card: &str) {
        for line in lines {
            assert!(
                line.width() <= width,
                "{:?} is {} cells, wider than the {card}",
                text(line),
                line.width()
            );
        }
    }

    #[test]
    fn the_help_fits_its_width() {
        let wide = help::HelpLayout::breakpoint();
        for width in [wide, wide + 1, 120] {
            let help = help::HelpLayout::at(width);
            fits(&help.lines(), help.inner, "help");
        }
        // The narrow card fits whole down to its own width plus the margins.
        let narrow = help::HelpLayout::at(wide - 1);
        let snug = narrow.width + 2 * MARGIN;
        assert!(snug < wide);
        for width in [wide - 1, snug] {
            let help = help::HelpLayout::at(width);
            fits(&help.lines(), help.inner, "narrow help");
            assert_eq!(help.inner, inner(help.width));
        }
        let squeezed = help::HelpLayout::at(snug - 1);
        assert!(squeezed.inner < inner(squeezed.width));
    }

    #[test]
    fn the_launch_card_fits_its_width() {
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
    }

    #[test]
    fn the_remove_card_fits_its_width() {
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
