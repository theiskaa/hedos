//! The task strip: the rows for background work and for what ran in the
//! foreground, oldest first, each expiring a while after it finished. Owns
//! the one invariant the reducer used to keep by hand: a finished row knows
//! the tick it finished on.

use super::tasks::{TaskEvent, TaskId, TaskKind, TaskLabel, TaskState};

/// How long a finished task stays in the strip, and how long a failed one
/// does, in ticks.
pub(super) const DONE_LINGER_TICKS: u64 = 60 * super::app::TICKS_PER_SECOND;
pub(super) const FAILED_LINGER_TICKS: u64 = 10 * 60 * super::app::TICKS_PER_SECOND;

/// A task as the strip shows it. A row for something that ran in the
/// foreground while the UI stepped aside has no kind: nothing spawned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: TaskId,
    pub label: TaskLabel,
    pub kind: Option<TaskKind>,
    pub state: TaskState,
    /// The tick the task finished on, for expiry.
    finished_at: Option<u64>,
}

impl TaskRow {
    /// Whether the task is still going.
    pub fn running(&self) -> bool {
        self.state.running()
    }
}

/// The strip's rows.
#[derive(Debug, Default)]
pub struct TaskStrip {
    rows: Vec<TaskRow>,
}

impl TaskStrip {
    /// The rows, oldest first.
    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }

    /// A row for `kind`, which the loop just started as task `id`.
    pub fn start(&mut self, id: TaskId, kind: TaskKind) {
        self.rows.push(TaskRow {
            id,
            label: kind.label(),
            kind: Some(kind),
            state: TaskState::Running,
            finished_at: None,
        });
    }

    /// A row for something that already ran, in the foreground, ending on
    /// tick `now`.
    pub fn record(&mut self, label: TaskLabel, state: TaskState, now: u64) {
        self.rows.push(TaskRow {
            id: TaskId::next(),
            label,
            kind: None,
            state,
            finished_at: Some(now),
        });
    }

    /// Apply a task's progress on tick `now`; the row it moved, if the task
    /// is in the strip.
    pub fn moved(&mut self, event: TaskEvent, now: u64) -> Option<&TaskRow> {
        let row = self.rows.iter_mut().find(|row| row.id == event.id)?;
        row.state = event.state;
        if !row.running() {
            row.finished_at = Some(now);
        }
        Some(row)
    }

    /// Drop the rows whose time is up on tick `now`; whether any was.
    pub fn expire(&mut self, now: u64) -> bool {
        let before = self.rows.len();
        self.rows.retain(|row| {
            row.finished_at.is_none_or(|finished| {
                let linger = match row.state {
                    TaskState::Failed(_) => FAILED_LINGER_TICKS,
                    _ => DONE_LINGER_TICKS,
                };
                now < finished + linger
            })
        });
        self.rows.len() != before
    }

    /// Drop the newest failed row; whether there was one.
    pub fn dismiss_newest_failure(&mut self) -> bool {
        let Some(id) = self.newest_failure() else {
            return false;
        };
        self.rows.retain(|row| row.id != id);
        true
    }

    /// Whether any task is still running.
    pub fn busy(&self) -> bool {
        self.rows.iter().any(TaskRow::running)
    }

    /// Whether a running task concerns model `id`.
    pub fn running_on(&self, id: &str) -> bool {
        self.rows
            .iter()
            .any(|row| row.running() && row.kind.as_ref().and_then(TaskKind::model_id) == Some(id))
    }

    /// The references of the pulls still running.
    pub fn pulling(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.running())
            .filter_map(|row| match &row.kind {
                Some(TaskKind::Pull(plan)) => Some(plan.reference.clone()),
                _ => None,
            })
            .collect()
    }

    /// Whether a task of `kind`'s shape is already running. Pulls match on
    /// what they fetch: two plans for one reference are one download.
    pub fn already_running(&self, kind: &TaskKind) -> bool {
        self.rows.iter().any(|row| {
            row.running()
                && match (&row.kind, kind) {
                    (Some(TaskKind::Pull(running)), TaskKind::Pull(wanted)) => {
                        running.provider == wanted.provider && running.reference == wanted.reference
                    }
                    (Some(running), wanted) => running == wanted,
                    (None, _) => false,
                }
        })
    }

    /// The rows a strip `height` rows tall shows, in their order: every
    /// running row, and the newest finished rows in what room is left. More
    /// running rows than fit keep the newest.
    pub fn shown(&self, height: usize) -> Vec<&TaskRow> {
        let running = self.rows.iter().filter(|row| row.running()).count();
        let mut finished_room = height.saturating_sub(running);
        let mut running_room = height.min(running);
        let mut kept = Vec::new();
        for row in self.rows.iter().rev() {
            let room = if row.running() {
                &mut running_room
            } else {
                &mut finished_room
            };
            if *room > 0 {
                *room -= 1;
                kept.push(row);
            }
        }
        kept.reverse();
        kept
    }

    /// The newest failed row, the one `d` dismisses.
    pub fn newest_failure(&self) -> Option<TaskId> {
        self.rows
            .iter()
            .rev()
            .find(|row| matches!(row.state, TaskState::Failed(_)))
            .map(|row| row.id)
    }

    /// The newest failed row when a strip `height` rows tall shows it: the
    /// one `d` acts on, and the one that carries the hint. A failure under
    /// more rows than fit shows no hint and does not go.
    pub fn visible_failure(&self, height: usize) -> Option<TaskId> {
        let shown = self.shown(height);
        self.newest_failure()
            .filter(|id| shown.iter().any(|row| row.id == *id))
    }

    /// The newest pull still running, resolving or downloading.
    pub fn newest_running_pull(&self) -> Option<TaskId> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.running() && matches!(row.kind, Some(TaskKind::Pull(_))))
            .map(|row| row.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_with(kind: TaskKind) -> (TaskStrip, TaskId) {
        let mut strip = TaskStrip::default();
        let id = TaskId::next();
        strip.start(id, kind);
        (strip, id)
    }

    #[test]
    fn a_failed_row_lingers_ten_times_longer_than_a_done_one() {
        let (mut strip, id) = strip_with(TaskKind::Scan);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Done("ok".to_owned()),
            },
            10,
        );
        assert!(!strip.expire(10 + DONE_LINGER_TICKS - 1));
        assert!(strip.expire(10 + DONE_LINGER_TICKS));
        let (mut strip, id) = strip_with(TaskKind::Scan);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Failed("no".to_owned()),
            },
            0,
        );
        assert!(!strip.expire(DONE_LINGER_TICKS));
        assert_eq!(strip.newest_failure(), Some(id));
        assert!(strip.dismiss_newest_failure());
        assert_eq!(strip.newest_failure(), None);
        assert!(!strip.dismiss_newest_failure());
    }

    #[test]
    fn running_rows_answer_the_guards() {
        let kind = TaskKind::Warm {
            id: "m".to_owned(),
            name: "m".to_owned(),
        };
        let (mut strip, id) = strip_with(kind.clone());
        assert!(strip.busy() && strip.running_on("m") && strip.already_running(&kind));
        assert!(!strip.running_on("other"));
        assert_eq!(strip.newest_running_pull(), None);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Done("warm".to_owned()),
            },
            1,
        );
        assert!(!strip.busy() && !strip.running_on("m") && !strip.already_running(&kind));
        strip.record(
            TaskLabel {
                verb: "chat",
                subject: "m".to_owned(),
            },
            TaskState::Done("ran 2s".to_owned()),
            5,
        );
        assert_eq!(strip.rows().len(), 2);
        assert!(strip.rows()[1].kind.is_none());
    }
}
