//! Where each pane goes for a given terminal size. Pure rect math, so the
//! breakpoints are tested without a terminal.

use ratatui::layout::{Constraint, Layout, Rect};

/// Below this many columns the detail pane stacks under the shelf.
const WIDE_COLUMNS: u16 = 100;
/// Height of the detail pane when stacked.
const STACKED_DETAIL_ROWS: u16 = 4;
/// Share of the width the shelf takes when side by side.
const SHELF_PERCENT: u16 = 55;
/// From this many rows the koala header, a row taller than the koala, earns
/// its place; below it the header is one line.
const TALL_ROWS: u16 = 40;
/// The koala header needs room for the koala and a panel beside it.
const TALL_COLUMNS: u16 = 70;
/// Rows of the koala header: the koala plus a blank line under it.
pub(crate) const TALL_HEADER_ROWS: u16 = crate::support::banner::KOALA.len() as u16 + 1;

/// The panes of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    /// The one-line header.
    pub header: Rect,
    /// The shelf table.
    pub shelf: Rect,
    /// The selected model's detail.
    pub detail: Rect,
    /// The one-line key footer.
    pub footer: Rect,
}

impl Panes {
    /// Split `area` into panes: a one-line header, the shelf beside or above
    /// the detail pane, and a one-line footer.
    pub fn compute(area: Rect) -> Self {
        let header_rows = if Self::tall(area) {
            TALL_HEADER_ROWS
        } else {
            1
        };
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(header_rows),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);
        let [shelf, detail] = if area.width >= WIDE_COLUMNS {
            Layout::horizontal([Constraint::Percentage(SHELF_PERCENT), Constraint::Min(0)])
                .areas(body)
        } else {
            Layout::vertical([Constraint::Min(0), Constraint::Length(STACKED_DETAIL_ROWS)])
                .areas(body)
        };
        Self {
            header,
            shelf,
            detail,
            footer,
        }
    }

    /// Whether `area` gets the koala header.
    pub fn tall(area: Rect) -> bool {
        area.height >= TALL_ROWS && area.width >= TALL_COLUMNS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(width: u16, height: u16) -> Panes {
        Panes::compute(Rect::new(0, 0, width, height))
    }

    #[test]
    fn a_wide_terminal_puts_the_detail_beside_the_shelf() {
        let panes = panes(110, 32);
        assert_eq!(panes.shelf.y, panes.detail.y);
        assert_eq!(panes.shelf.width + panes.detail.width, 110);
        assert_eq!(panes.header.height, 1);
        assert_eq!(panes.footer.y, 31);
    }

    #[test]
    fn a_narrow_terminal_stacks_the_detail_under_the_shelf() {
        let panes = panes(80, 30);
        assert_eq!(panes.shelf.x, panes.detail.x);
        assert_eq!(panes.detail.y, panes.shelf.y + panes.shelf.height);
        assert_eq!(panes.detail.height, STACKED_DETAIL_ROWS);
    }

    #[test]
    fn the_koala_header_needs_height_and_width() {
        assert_eq!(panes(110, 44).header.height, TALL_HEADER_ROWS);
        assert_eq!(panes(110, 32).header.height, 1);
        assert_eq!(panes(60, 44).header.height, 1);
    }

    #[test]
    fn tiny_terminals_never_panic() {
        for (width, height) in [(0, 0), (1, 1), (40, 3), (20, 60)] {
            let _ = panes(width, height);
        }
    }
}
