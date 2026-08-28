//! The launch card: every harness, the ones this model can seat selectable,
//! the rest dim with the reason.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};

use super::BORDER_ROWS;
use crate::support::harnesses::HARNESSES;
use crate::tui::launch::LaunchModal;
use crate::tui::text;
use crate::tui::ui::{DIM, SELECTED_ROW, keys, label_width, padded};

/// The launch modal's width: a harness, its binary, and the reason it is
/// blocked, clipped to fit.
pub(super) const LAUNCH_WIDTH: u16 = 72;
/// The launch modal: a blank, one row per harness, a blank, the note, a
/// blank, the keys, and the border.
pub(super) const LAUNCH_HEIGHT: u16 = HARNESSES.len() as u16 + 5 + BORDER_ROWS;

/// Every harness, the ones this model can seat selectable, the rest dim with
/// the reason, clipped to what `inner` leaves after the two columns.
pub(super) fn launch(modal: &LaunchModal, inner: Rect) -> Vec<Line<'static>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tui::testing::{record_with, texts};

    #[test]
    fn the_card_holds_a_row_per_harness_and_the_keys() {
        let modal = LaunchModal::open_with(&record_with("m", Vec::new()), |_| None);
        let lines = launch(&modal, Rect::new(0, 0, LAUNCH_WIDTH - 2, LAUNCH_HEIGHT));
        assert_eq!(lines.len() as u16, LAUNCH_HEIGHT - BORDER_ROWS);
        let shown = texts(&lines);
        for spec in HARNESSES {
            assert!(
                shown.iter().any(|line| line.contains(spec.display)),
                "{} is not listed",
                spec.display
            );
        }
        assert_eq!(
            shown.last().map(|line| line.trim_end()),
            Some(" enter launch  ↑/↓ move  esc close")
        );
    }
}
