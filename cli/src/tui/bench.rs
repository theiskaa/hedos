//! The bench screen: what each model does on this machine, measured from the
//! shelf rather than from a command.
//!
//! The screen takes the shelf's place while it is open, so it has a selection
//! of its own and the detail pane follows it. The rows and their figures are
//! the same [`Board`](crate::support::bench_view::Board) the `hedos bench`
//! command keeps, fed by the same driver, so the two surfaces cannot disagree
//! about a number. Results live for as long as the screen does; a bench in
//! flight goes on while the shelf is showing, and the screen picks it back up.

use kernel::bench::{Row, Status};
use runtime::bench::BenchEvent;

use crate::support::bench_view::Board;

/// The screen's state.
#[derive(Debug, Default)]
pub(crate) struct BenchScreen {
    /// The bench being watched, once one has been started.
    board: Option<Board>,
    /// The row the cursor is on. The screen draws its own lines rather than a
    /// stateful table, and scrolls them itself, so an index is the whole of it.
    selected: usize,
    /// Whether a bench is still going.
    running: bool,
    /// Which bench the rows belong to; a step stamped with an older one is a
    /// message from a bench already replaced and is dropped.
    generation: u64,
    /// Whether the selection follows the row being measured. It does until the
    /// user moves it themselves, which is taken to mean they are reading.
    following: bool,
}

impl BenchScreen {
    /// The board, once a bench has been started.
    pub(crate) fn board(&self) -> Option<&Board> {
        self.board.as_ref()
    }

    /// The rows, empty until a bench has been started.
    pub(crate) fn rows(&self) -> &[Row] {
        self.board.as_ref().map_or(&[], |board| &board.rows)
    }

    /// Whether a bench is still going.
    pub(crate) fn running(&self) -> bool {
        self.running
    }

    /// The index of the selected row.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// The selected row, if there is one.
    pub(crate) fn selected_row(&self) -> Option<&Row> {
        self.rows().get(self.selected())
    }

    /// Take `board` as the bench now running, under `generation`, selecting its
    /// first row and following the measurements again.
    pub(crate) fn start(&mut self, board: Board, generation: u64) {
        self.board = Some(board);
        self.running = true;
        self.generation = generation;
        self.selected = 0;
        self.following = true;
    }

    /// Queue `ids` on the board already shown, keeping every other row's
    /// figures, so measuring one model again does not throw away the table it
    /// was going to be compared against. `false` when there is no such board.
    pub(crate) fn requeue(&mut self, ids: &[String], generation: u64) -> bool {
        let Some(board) = self.board.as_mut() else {
            return false;
        };
        if !ids
            .iter()
            .all(|id| board.rows.iter().any(|row| &row.id == id))
        {
            return false;
        }
        for row in &mut board.rows {
            if ids.contains(&row.id) {
                row.status = Status::Waiting;
            }
        }
        self.running = true;
        self.generation = generation;
        self.following = true;
        true
    }

    /// Fold in one step of the bench; whether anything the screen shows moved.
    /// The last row settling is what ends the run, so nothing else has to be
    /// told the bench is over.
    pub(crate) fn apply(&mut self, generation: u64, event: &BenchEvent) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(board) = self.board.as_mut() else {
            return false;
        };
        let moved = board.apply(event);
        if board.finished() {
            self.running = false;
        }
        moved
    }

    /// Move the selection by `delta` rows; whether it moved.
    pub(crate) fn step(&mut self, delta: isize) -> bool {
        let selected = self.selected() as isize + delta;
        self.following = false;
        self.select(selected.max(0) as usize)
    }

    /// Put the selection on `index`, clamped to the last row; whether it moved.
    pub(crate) fn select(&mut self, index: usize) -> bool {
        let rows = self.rows().len();
        if rows == 0 {
            return false;
        }
        let index = index.min(rows - 1);
        if self.selected == index {
            return false;
        }
        self.selected = index;
        true
    }

    /// Put the selection on the row being measured, so an unattended screen
    /// follows the bench. Only while nothing has been chosen by hand: a
    /// selection the user moved is one they are reading, and dragging it back
    /// eight times a second would make the screen unusable during a bench.
    pub(crate) fn follow_running(&mut self) -> bool {
        if !self.following {
            return false;
        }
        let Some(running) = self.board.as_ref().and_then(Board::running) else {
            return false;
        };
        self.select(running)
    }

    /// Settle every row that never ran, for a bench that was stopped. A
    /// generation the screen has moved past is a driver returning after its
    /// bench was replaced, and settles nothing.
    pub(crate) fn stopped(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.running = false;
        let Some(board) = self.board.as_mut() else {
            return;
        };
        for row in &mut board.rows {
            if matches!(row.status, Status::Waiting | Status::Running { .. }) {
                row.status = Status::Stopped;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::bench::Phase;

    fn screen(ids: &[&str]) -> BenchScreen {
        let rows = ids
            .iter()
            .map(|id| Row::waiting(*id, *id, None, None))
            .collect();
        let mut screen = BenchScreen::default();
        screen.start(Board::new(rows, 3, 128, "test".to_owned()), 1);
        screen
    }

    fn settle(id: &str) -> BenchEvent {
        BenchEvent::Settled {
            id: id.to_owned(),
            status: Box::new(Status::Stopped),
        }
    }

    #[test]
    fn a_started_bench_is_running_until_its_last_row_settles() {
        let mut screen = screen(&["a", "b"]);
        assert!(screen.running());
        assert!(screen.apply(1, &settle("a")));
        assert!(screen.running(), "one row is still waiting");
        screen.apply(1, &settle("b"));
        assert!(!screen.running());
    }

    #[test]
    fn the_selection_moves_within_the_rows_and_no_further() {
        let mut screen = screen(&["a", "b"]);
        assert!(screen.step(1));
        assert_eq!(screen.selected(), 1);
        assert!(!screen.step(1), "the last row is the floor");
        assert!(screen.step(-1));
        assert_eq!(
            screen.selected_row().map(|row| row.name.as_str()),
            Some("a")
        );
    }

    #[test]
    fn following_puts_the_selection_on_the_row_being_measured() {
        let mut screen = screen(&["a", "b"]);
        screen.apply(
            1,
            &BenchEvent::Started {
                id: "b".to_owned(),
                phase: Phase::ColdStart,
            },
        );
        assert!(screen.follow_running());
        assert_eq!(screen.selected(), 1);
    }

    #[test]
    fn stopping_settles_every_row_that_never_ran() {
        let mut screen = screen(&["a", "b"]);
        screen.stopped(1);
        assert!(!screen.running());
        assert!(
            screen
                .rows()
                .iter()
                .all(|row| row.status == Status::Stopped)
        );
    }

    #[test]
    fn a_step_from_a_bench_already_replaced_is_dropped() {
        let mut screen = screen(&["a", "b"]);
        screen.start(
            Board::new(
                vec![Row::waiting("a", "a", None, None)],
                3,
                128,
                "test".to_owned(),
            ),
            2,
        );
        assert!(!screen.apply(1, &settle("a")), "the old bench's step");
        assert!(screen.running(), "and it did not end the new one");
        screen.stopped(1);
        assert!(screen.running(), "nor stop it");
        assert!(screen.apply(2, &settle("a")));
        assert!(!screen.running());
    }

    #[test]
    fn measuring_one_row_again_keeps_the_rest_of_the_table() {
        let mut screen = screen(&["a", "b"]);
        screen.apply(1, &settle("a"));
        screen.apply(1, &settle("b"));
        assert!(screen.requeue(&["a".to_owned()], 2));
        assert_eq!(screen.rows()[0].status, Status::Waiting);
        assert_eq!(
            screen.rows()[1].status,
            Status::Stopped,
            "b keeps its figure"
        );
        assert!(
            !screen.requeue(&["ghost".to_owned()], 3),
            "a row it has not got"
        );
    }

    #[test]
    fn a_selection_the_user_moved_is_not_dragged_back_to_the_running_row() {
        let mut screen = screen(&["a", "b"]);
        screen.apply(
            1,
            &BenchEvent::Started {
                id: "b".to_owned(),
                phase: Phase::ColdStart,
            },
        );
        assert!(screen.follow_running(), "it follows on its own");
        screen.step(-1);
        assert_eq!(screen.selected(), 0);
        assert!(!screen.follow_running());
        assert_eq!(screen.selected(), 0, "and stays where it was put");
    }

    #[test]
    fn a_screen_with_no_bench_yet_answers_empty() {
        let screen = BenchScreen::default();
        assert!(screen.rows().is_empty());
        assert!(screen.selected_row().is_none());
        assert!(!screen.running());
    }
}
