//! The install card: what installing llama.cpp fetches, from where, how big
//! and where it goes, said before anything is fetched.

use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::tui::app::InstallCard;
use crate::tui::text;
use crate::tui::ui::{BORDER_ROWS, keys};

/// The install card's width: a release URL, elided to fit when it is longer.
pub(super) const INSTALL_WIDTH: u16 = 100;

/// The install card's height: a blank, its lines, a blank, the keys, and the
/// border.
pub(super) fn height(card: &InstallCard) -> u16 {
    u16::try_from(card.lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(3 + BORDER_ROWS)
}

/// What installing llama.cpp does, and the keys that answer.
pub(super) fn install(card: &InstallCard, inner: Rect) -> Vec<Line<'static>> {
    let width = usize::from(inner.width).saturating_sub(1);
    let mut lines = vec![Line::default()];
    lines.extend(
        card.lines
            .iter()
            .map(|line| Line::from(format!(" {}", text::elide_middle(line, width)))),
    );
    lines.push(Line::default());
    lines.push(keys(&[("y", "install"), ("n", "not now")]));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::testing::texts;

    #[test]
    fn the_card_says_what_is_fetched_and_fits_its_height() {
        let card = InstallCard {
            lines: vec![
                "llama.cpp 0.7.0 (b11669), the metal build".to_owned(),
                format!("  from     https://example.com/{}", "a".repeat(200)),
            ],
            variant: kernel::engines::llama_cpp::Variant::Metal,
        };
        let lines = install(&card, Rect::new(0, 0, INSTALL_WIDTH - 2, 10));
        assert_eq!(lines.len() as u16, height(&card) - BORDER_ROWS);
        let shown = texts(&lines);
        assert!(shown[1].contains("metal build"));
        assert!(shown[2].contains('…'));
        assert!(shown.last().is_some_and(|keys| keys.contains("install")));
    }
}
