//! Where each pane goes for a given terminal size. Pure rect math, so the
//! breakpoints are tested without a terminal.

use ratatui::layout::{Constraint, Layout, Rect};

/// Below this many columns the detail pane stacks under the shelf.
const WIDE_COLUMNS: u16 = 100;
/// Height of the detail pane when stacked.
const STACKED_DETAIL_ROWS: u16 = 6;
/// Share of the width the shelf takes when side by side.
const SHELF_PERCENT: u16 = 55;
/// From this many rows the koala header earns its place; below it the header
/// is one line.
const TALL_ROWS: u16 = 44;
/// The koala header needs room for the koala and a panel beside it.
const TALL_COLUMNS: u16 = 70;
/// Rows of the koala header: the koala plus a blank line under it.
pub(crate) const TALL_HEADER_ROWS: u16 = crate::support::banner::KOALA.len() as u16 + 1;
/// The most task rows the strip shows at once.
const MAX_TASK_ROWS: u16 = 4;
/// Rows of the machine block: a border, memory, legend or disk, a border.
const MACHINE_ROWS: u16 = 5;
/// Rows of the gateway block under the detail: a border, two lines, a border.
const GATEWAY_ROWS: u16 = 4;
/// The shelf never shrinks below this many rows, borders included.
const MIN_SHELF_ROWS: u16 = 6;
/// Rows the shelf's chrome takes: two borders and the column header.
const SHELF_CHROME_ROWS: u16 = 3;

/// The panes of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    /// The header, one line or the koala.
    pub header: Rect,
    /// The shelf table.
    pub shelf: Rect,
    /// The selected model's detail.
    pub detail: Rect,
    /// The machine facts under the shelf; zero-height when there is no room.
    pub machine: Rect,
    /// The gateway facts beside the machine block; zero-width when stacked.
    pub gateway: Rect,
    /// The task strip; zero-height when there are no tasks.
    pub tasks: Rect,
    /// The one-line key footer.
    pub footer: Rect,
}

impl Panes {
    /// Split `area` into panes for a shelf of `shelf_rows` models, `task_rows`
    /// tasks, and the detail alone when `expanded`.
    pub fn compute(area: Rect, shelf_rows: usize, task_rows: usize, expanded: bool) -> Self {
        let header_rows = if Self::tall(area) {
            TALL_HEADER_ROWS
        } else {
            1
        };
        let strip_rows = match task_rows {
            0 => 0,
            rows => (rows as u16).min(MAX_TASK_ROWS) + 2,
        };
        let [header, body, tasks, footer] = Layout::vertical([
            Constraint::Length(header_rows),
            Constraint::Min(0),
            Constraint::Length(strip_rows),
            Constraint::Length(1),
        ])
        .areas(area);

        if expanded {
            return Self {
                header,
                shelf: Rect::default(),
                detail: body,
                machine: Rect::default(),
                gateway: Rect::default(),
                tasks,
                footer,
            };
        }

        let wanted = (shelf_rows as u16)
            .saturating_add(SHELF_CHROME_ROWS)
            .max(MIN_SHELF_ROWS);
        if area.width < WIDE_COLUMNS {
            // Stacked, the detail and the machine block each cost rows; drop
            // them from the bottom up rather than squeeze the shelf below its
            // floor.
            let detail_rows = if body.height >= MIN_SHELF_ROWS + STACKED_DETAIL_ROWS {
                STACKED_DETAIL_ROWS
            } else {
                0
            };
            let machine_rows = if body.height >= MIN_SHELF_ROWS + STACKED_DETAIL_ROWS + MACHINE_ROWS
            {
                MACHINE_ROWS
            } else {
                0
            };
            let [shelf, detail, machine] = Layout::vertical([
                Constraint::Min(0),
                Constraint::Length(detail_rows),
                Constraint::Length(machine_rows),
            ])
            .areas(body);
            return Self {
                header,
                shelf,
                detail,
                machine,
                gateway: Rect::default(),
                tasks,
                footer,
            };
        }
        // A long shelf scrolls rather than pushing the machine facts off;
        // only a terminal too short for both loses the block.
        let with_machine = body.height >= MIN_SHELF_ROWS + MACHINE_ROWS;
        let machine_rows = if with_machine { MACHINE_ROWS } else { 0 };
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(SHELF_PERCENT), Constraint::Min(0)])
                .areas(body);
        // The shelf takes the rows it needs, the machine block sits right
        // under it, and the slack is left at the bottom of the column.
        let shelf_rows = if with_machine {
            wanted.min(left.height.saturating_sub(machine_rows))
        } else {
            left.height
        };
        let [shelf, machine, _] = Layout::vertical([
            Constraint::Length(shelf_rows),
            Constraint::Length(machine_rows),
            Constraint::Min(0),
        ])
        .areas(left);
        let [detail, gateway] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(if with_machine { GATEWAY_ROWS } else { 0 }),
        ])
        .areas(right);
        Self {
            header,
            shelf,
            detail,
            machine,
            gateway,
            tasks,
            footer,
        }
    }

    /// Whether `area` gets the koala header.
    fn tall(area: Rect) -> bool {
        area.height >= TALL_ROWS && area.width >= TALL_COLUMNS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panes(width: u16, height: u16) -> Panes {
        Panes::compute(Rect::new(0, 0, width, height), 14, 0, false)
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
    fn the_machine_block_sits_right_under_the_shelf() {
        let panes = panes(110, 32);
        assert_eq!(panes.shelf.height, 14 + SHELF_CHROME_ROWS);
        assert_eq!(panes.machine.y, panes.shelf.y + panes.shelf.height);
        assert_eq!(panes.machine.height, MACHINE_ROWS);
        assert_eq!(panes.gateway.x, panes.detail.x);
        assert_eq!(panes.gateway.y + GATEWAY_ROWS, panes.footer.y);
        assert_eq!(
            panes.detail.height + GATEWAY_ROWS,
            panes.footer.y - panes.detail.y
        );
    }

    #[test]
    fn a_long_shelf_keeps_the_machine_block_while_it_fits() {
        let panes = Panes::compute(Rect::new(0, 0, 110, 32), 40, 0, false);
        assert_eq!(panes.machine.height, MACHINE_ROWS);
        assert_eq!(panes.shelf.height, 30 - MACHINE_ROWS);
        let short = Panes::compute(Rect::new(0, 0, 110, 12), 40, 0, false);
        assert_eq!(short.machine.height, 0);
        assert_eq!(short.shelf.height, 10);
    }

    #[test]
    fn a_narrow_terminal_stacks_the_detail_under_the_shelf() {
        let panes = panes(80, 30);
        assert_eq!(panes.shelf.x, panes.detail.x);
        assert_eq!(panes.detail.y, panes.shelf.y + panes.shelf.height);
        assert_eq!(panes.detail.height, STACKED_DETAIL_ROWS);
        assert_eq!(panes.gateway.width, 0);
        assert_eq!(panes.machine.height, MACHINE_ROWS);
    }

    #[test]
    fn a_short_narrow_terminal_keeps_the_shelf_floor() {
        let panes = Panes::compute(Rect::new(0, 0, 80, 13), 14, 0, false);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.detail.height, 0);
        assert_eq!(panes.shelf.height, 11);
        let roomier = Panes::compute(Rect::new(0, 0, 80, 20), 14, 0, false);
        assert_eq!(roomier.detail.height, STACKED_DETAIL_ROWS);
        assert_eq!(roomier.machine.height, MACHINE_ROWS);
        assert!(roomier.shelf.height >= MIN_SHELF_ROWS);
    }

    #[test]
    fn a_short_wide_terminal_drops_the_machine_and_gateway_blocks() {
        let panes = Panes::compute(Rect::new(0, 0, 110, 12), 14, 0, false);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.gateway.height, 0);
        assert_eq!(panes.shelf.height, 10);
        let tiny = Panes::compute(Rect::new(0, 0, 110, 32), 1, 0, false);
        assert_eq!(tiny.shelf.height, MIN_SHELF_ROWS);
    }

    #[test]
    fn the_koala_header_needs_height_and_width() {
        assert_eq!(panes(110, 44).header.height, TALL_HEADER_ROWS);
        assert_eq!(panes(110, 32).header.height, 1);
        assert_eq!(panes(60, 44).header.height, 1);
    }

    #[test]
    fn the_task_strip_takes_rows_only_when_there_are_tasks() {
        let area = Rect::new(0, 0, 110, 32);
        assert_eq!(Panes::compute(area, 14, 0, false).tasks.height, 0);
        assert_eq!(Panes::compute(area, 14, 1, false).tasks.height, 3);
        assert_eq!(
            Panes::compute(area, 14, 9, false).tasks.height,
            MAX_TASK_ROWS + 2
        );
    }

    #[test]
    fn an_expanded_detail_takes_the_whole_body() {
        let panes = Panes::compute(Rect::new(0, 0, 110, 32), 14, 0, true);
        assert_eq!(panes.shelf.width, 0);
        assert_eq!(panes.machine.height, 0);
        assert_eq!(panes.detail.width, 110);
        assert_eq!(panes.detail.height, 30);
    }

    #[test]
    fn tiny_terminals_never_panic() {
        for (width, height) in [(0, 0), (1, 1), (40, 3), (20, 60)] {
            let _ = panes(width, height);
        }
    }
}
