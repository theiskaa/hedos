//! Where each pane goes for a given terminal size. Pure rect math, so the
//! breakpoints are tested without a terminal.
//!
//! The panes are cards on a painted ground: a column of margin on either
//! side of a wide terminal, a column of gutter between cards side by side,
//! and cards stacked edge to edge, as the rounded corners already part them.

use ratatui::layout::{Constraint, Layout, Rect};

use super::palette::BORDER_ROWS;

/// Below this many columns the detail pane stacks under the shelf.
const WIDE_WIDTH: u16 = 100;
/// Height of the detail pane when stacked; at or under it the detail shows
/// only what the shelf row does not.
pub(super) const STACKED_DETAIL_ROWS: u16 = 6;
/// Share of the width the shelf takes when side by side.
const SHELF_PERCENT: u16 = 59;
/// The hero header earns its place from this many rows and columns; below
/// either, the header is one line.
const HERO_MIN_ROWS: u16 = 40;
const HERO_MIN_WIDTH: u16 = 96;
/// Rows of the hero header: the koala with a row of air above and below.
pub(super) const HERO_ROWS: u16 = crate::support::banner::KOALA.len() as u16 + 2;
/// The most task rows the strip shows at once.
pub(super) const MAX_TASK_ROWS: u16 = 4;
/// Rows of the gateway card beside the machine card: a border, three lines,
/// a border.
const GATEWAY_ROWS: u16 = 5;
/// The shelf never shrinks below this many rows, borders included.
const MIN_SHELF_ROWS: u16 = 6;
/// Rows the shelf's chrome takes: two borders and the column header.
const SHELF_CHROME_ROWS: u16 = 3;
/// Stacked, the machine card shows only while the shelf keeps this many
/// rows, or all it wants when it wants fewer.
const STACKED_SHELF_FLOOR: u16 = 12;
/// Columns kept clear on either side of a wide terminal.
pub(super) const MARGIN: u16 = 1;
/// Columns between two cards side by side.
pub(super) const GUTTER: u16 = 1;

/// Whether `area` is narrow enough that the panes stack.
pub(crate) fn stacks(area: Rect) -> bool {
    area.width < WIDE_WIDTH
}

/// The panes of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    /// The header, one line or the hero.
    pub header: Rect,
    /// The shelf table.
    pub shelf: Rect,
    /// The selected model's detail.
    pub detail: Rect,
    /// The machine facts under the shelf; zero-height when there is no room.
    pub machine: Rect,
    /// The gateway facts beside the machine card; zero-width when stacked.
    pub gateway: Rect,
    /// The task strip; zero-height when there are no tasks.
    pub tasks: Rect,
    /// The one-line key footer.
    pub footer: Rect,
}

/// `area` less the margin on either side, which only a wide terminal keeps.
fn within_margins(area: Rect) -> Rect {
    let margin = if stacks(area) { 0 } else { MARGIN };
    Rect {
        x: area.x + margin.min(area.width / 2),
        width: area.width.saturating_sub(2 * margin),
        ..area
    }
}

/// The body under a narrow terminal: shelf, detail, machine card, top to
/// bottom. The detail and the machine card each cost rows; they are dropped
/// from the bottom up rather than squeezing the shelf below its floor.
fn stacked(body: Rect, shelf_rows: usize, machine_block: u16) -> (Rect, Rect, Rect, Rect) {
    // With nothing listed there is nothing to detail, and the empty shelf's
    // copy wants every row.
    let detail_rows = if shelf_rows > 0 && body.height >= MIN_SHELF_ROWS + STACKED_DETAIL_ROWS {
        STACKED_DETAIL_ROWS
    } else {
        0
    };
    let wanted = wanted_shelf_rows(shelf_rows);
    let floor = wanted.clamp(MIN_SHELF_ROWS, STACKED_SHELF_FLOOR);
    let machine_rows = if machine_block > 0
        && body.height >= floor + detail_rows + machine_block
        && detail_rows > 0
    {
        machine_block
    } else {
        0
    };
    let [shelf, detail, machine] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(detail_rows),
        Constraint::Length(machine_rows),
    ])
    .areas(body);
    (shelf, detail, machine, Rect::default())
}

/// The rows the shelf would take to show every one of `shelf_rows` models.
fn wanted_shelf_rows(shelf_rows: usize) -> u16 {
    u16::try_from(shelf_rows)
        .unwrap_or(u16::MAX)
        .saturating_add(SHELF_CHROME_ROWS)
        .max(MIN_SHELF_ROWS)
}

/// The two columns of a wide body: the left one at `SHELF_PERCENT` of what
/// the gutter leaves, the right one the rest.
fn columns(body: Rect) -> (Rect, Rect) {
    let usable = body.width.saturating_sub(GUTTER);
    let left = usable * SHELF_PERCENT / 100;
    let right = usable - left;
    (
        Rect {
            width: left,
            ..body
        },
        Rect {
            x: body.x + left + GUTTER,
            width: right,
            ..body
        },
    )
}

/// The body under a wide terminal: the shelf beside the detail, and under
/// them the machine card beside the gateway card at one shared height. A
/// long shelf scrolls rather than pushing the machine facts off; only a
/// terminal too short for both loses the bottom row.
fn side_by_side(body: Rect, machine_block: u16) -> (Rect, Rect, Rect, Rect) {
    let bottom = machine_block.max(GATEWAY_ROWS);
    let bottom = if machine_block > 0 && body.height >= MIN_SHELF_ROWS + bottom {
        bottom
    } else {
        0
    };
    let [top, under] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom)]).areas(body);
    let (shelf, detail) = columns(top);
    let (machine, gateway) = columns(under);
    (shelf, detail, machine, gateway)
}

impl Panes {
    /// Split `area` into panes for a shelf of `shelf_rows` models, a machine
    /// card of `machine_lines`, `task_rows` tasks, and the detail alone when
    /// `expanded`.
    pub fn compute(
        area: Rect,
        shelf_rows: usize,
        machine_lines: u16,
        task_rows: usize,
        expanded: bool,
    ) -> Self {
        Self::split(
            area,
            shelf_rows,
            machine_lines + BORDER_ROWS,
            task_rows,
            expanded,
        )
    }

    /// Split `area` for the pulls screen: the list of `pull_rows` jobs where
    /// the shelf goes and the selected pull's detail where the model's goes,
    /// with no machine or gateway card, since the screen is about the pulls
    /// and the header carries the machine's one line.
    pub fn pulls(area: Rect, pull_rows: usize, task_rows: usize) -> Self {
        Self::split(area, pull_rows, 0, task_rows, false)
    }

    /// Split `area` for the bench screen, which is laid out as the pulls one
    /// is: the list of models where the shelf goes, the selected row's figures
    /// where the model's detail goes.
    pub fn bench(area: Rect, bench_rows: usize, task_rows: usize) -> Self {
        Self::pulls(area, bench_rows, task_rows)
    }

    /// The layout behind every screen; a `machine_block` of zero rows means
    /// no machine or gateway card at all.
    fn split(
        area: Rect,
        shelf_rows: usize,
        machine_block: u16,
        task_rows: usize,
        expanded: bool,
    ) -> Self {
        let header_rows = if Self::hero(area) { HERO_ROWS } else { 1 };
        let strip_rows = match task_rows {
            0 => 0,
            rows => u16::try_from(rows).unwrap_or(u16::MAX).min(MAX_TASK_ROWS) + BORDER_ROWS,
        };
        let [header, body, tasks, footer] = Layout::vertical([
            Constraint::Length(header_rows),
            Constraint::Min(0),
            Constraint::Length(strip_rows),
            Constraint::Length(1),
        ])
        .areas(area);
        let body = within_margins(body);
        let (shelf, detail, machine, gateway) = if expanded {
            (Rect::default(), body, Rect::default(), Rect::default())
        } else if stacks(area) {
            stacked(body, shelf_rows, machine_block)
        } else {
            side_by_side(body, machine_block)
        };
        Self {
            header,
            shelf,
            detail,
            machine,
            gateway,
            tasks: within_margins(tasks),
            footer,
        }
    }

    /// Whether `area` gets the hero header.
    pub(crate) fn hero(area: Rect) -> bool {
        area.height >= HERO_MIN_ROWS && area.width >= HERO_MIN_WIDTH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: u16 = 3 + BORDER_ROWS;

    fn panes(width: u16, height: u16) -> Panes {
        Panes::compute(Rect::new(0, 0, width, height), 14, 3, 0, false)
    }

    /// Every rect in `panes` that has any area.
    fn rects(panes: &Panes) -> Vec<Rect> {
        [
            panes.shelf,
            panes.detail,
            panes.machine,
            panes.gateway,
            panes.tasks,
        ]
        .into_iter()
        .filter(|rect| rect.area() > 0)
        .collect()
    }

    #[test]
    fn the_pulls_screen_has_no_machine_or_gateway_card() {
        let wide = Panes::pulls(Rect::new(0, 0, 110, 32), 3, 0);
        assert_eq!(wide.machine.height, 0);
        assert_eq!(wide.gateway.height, 0);
        assert_eq!(wide.shelf.y, wide.detail.y);
        assert_eq!(wide.shelf.height, wide.detail.height);
        assert_eq!(wide.detail.height, wide.footer.y - wide.detail.y);
        let narrow = Panes::pulls(Rect::new(0, 0, 80, 30), 3, 0);
        assert_eq!(narrow.machine.height, 0);
        assert_eq!(narrow.detail.height, STACKED_DETAIL_ROWS);
        assert_eq!(
            narrow.shelf.height + narrow.detail.height,
            narrow.footer.y - narrow.shelf.y
        );
    }

    #[test]
    fn a_wide_terminal_puts_the_detail_beside_the_shelf_inside_the_margins() {
        let panes = panes(110, 32);
        assert_eq!(panes.shelf.y, panes.detail.y);
        assert_eq!(panes.shelf.height, panes.detail.height);
        assert_eq!(panes.shelf.x, MARGIN);
        assert_eq!(
            panes.shelf.width + GUTTER + panes.detail.width + 2 * MARGIN,
            110
        );
        assert_eq!(panes.detail.x, panes.shelf.x + panes.shelf.width + GUTTER);
        assert_eq!(panes.header.height, 1);
        assert_eq!(panes.footer.y, 31);
    }

    #[test]
    fn the_shelf_takes_the_mocks_share_at_132_columns() {
        // The one literal pin: 76 of the 129 columns the margins and gutter
        // leave at 132.
        assert_eq!(panes(132, 42).shelf.width, 76);
    }

    #[test]
    fn the_machine_and_gateway_cards_share_the_bottom_row() {
        let panes = panes(110, 32);
        assert_eq!(panes.machine.height, MACHINE.max(GATEWAY_ROWS));
        assert_eq!(panes.machine.y, panes.gateway.y);
        assert_eq!(panes.machine.height, panes.gateway.height);
        assert_eq!(panes.machine.x, panes.shelf.x);
        assert_eq!(panes.gateway.x, panes.detail.x);
        assert_eq!(panes.machine.y, panes.shelf.y + panes.shelf.height);
        assert_eq!(panes.gateway.y + panes.gateway.height, panes.footer.y);
    }

    #[test]
    fn a_short_wide_terminal_drops_the_bottom_row() {
        let panes = Panes::compute(Rect::new(0, 0, 110, 12), 14, 3, 0, false);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.gateway.height, 0);
        assert_eq!(panes.shelf.height, 10);
    }

    #[test]
    fn a_narrow_terminal_stacks_the_detail_under_the_shelf() {
        let panes = Panes::compute(Rect::new(0, 0, 80, 30), 3, 3, 0, false);
        assert_eq!(panes.shelf.x, 0);
        assert_eq!(panes.shelf.width, 80);
        assert_eq!(panes.detail.y, panes.shelf.y + panes.shelf.height);
        assert_eq!(panes.detail.height, STACKED_DETAIL_ROWS);
        assert_eq!(panes.gateway.width, 0);
        assert_eq!(panes.machine.height, MACHINE);
    }

    #[test]
    fn a_long_stacked_shelf_keeps_its_rows_before_the_machine_card() {
        let full = Panes::compute(Rect::new(0, 0, 80, 24), 15, 3, 0, false);
        assert_eq!(full.machine.height, 0);
        assert_eq!(full.detail.height, STACKED_DETAIL_ROWS);
        assert!(full.shelf.height >= STACKED_SHELF_FLOOR);
        let short = Panes::compute(Rect::new(0, 0, 80, 24), 3, 3, 0, false);
        assert_eq!(short.machine.height, MACHINE);
    }

    #[test]
    fn a_short_narrow_terminal_keeps_the_shelf_floor() {
        let panes = Panes::compute(Rect::new(0, 0, 80, 13), 14, 3, 0, false);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.detail.height, 0);
        assert_eq!(panes.shelf.height, 11);
        let roomier = Panes::compute(Rect::new(0, 0, 80, 20), 3, 3, 0, false);
        assert_eq!(roomier.detail.height, STACKED_DETAIL_ROWS);
        assert!(roomier.shelf.height >= MIN_SHELF_ROWS);
    }

    #[test]
    fn the_hero_needs_height_and_width() {
        assert_eq!(panes(96, 40).header.height, HERO_ROWS);
        assert_eq!(panes(95, 40).header.height, 1);
        assert_eq!(panes(132, 39).header.height, 1);
    }

    #[test]
    fn the_task_strip_spans_both_columns_inside_the_margins() {
        let area = Rect::new(0, 0, 110, 32);
        assert_eq!(Panes::compute(area, 14, 3, 0, false).tasks.height, 0);
        let one = Panes::compute(area, 14, 3, 1, false);
        assert_eq!(one.tasks.height, 3);
        assert_eq!(one.tasks.x, MARGIN);
        assert_eq!(one.tasks.width, 110 - 2 * MARGIN);
        assert_eq!(
            Panes::compute(area, 14, 3, 9, false).tasks.height,
            MAX_TASK_ROWS + 2
        );
    }

    #[test]
    fn an_expanded_detail_takes_the_whole_body() {
        let panes = Panes::compute(Rect::new(0, 0, 110, 32), 14, 3, 0, true);
        assert_eq!(panes.shelf.width, 0);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.detail.width, 110 - 2 * MARGIN);
        assert_eq!(panes.detail.height, 30);
    }

    #[test]
    fn no_card_covers_a_margin_or_the_gutter() {
        for (width, height) in [(132, 42), (120, 35), (100, 30)] {
            let panes = panes(width, height);
            let gutter = panes.shelf.x + panes.shelf.width;
            for rect in rects(&panes) {
                assert!(rect.x >= MARGIN, "{rect:?} at {width}");
                assert!(rect.x + rect.width <= width - MARGIN, "{rect:?} at {width}");
                assert!(
                    gutter < rect.x || gutter >= rect.x + rect.width,
                    "{rect:?} covers the gutter at {width}"
                );
            }
        }
    }

    #[test]
    fn tiny_terminals_never_panic() {
        for (width, height) in [(0, 0), (1, 1), (40, 3), (20, 60)] {
            let _ = panes(width, height);
        }
    }
}
