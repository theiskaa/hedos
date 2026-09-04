//! The pulls screen: every job the store holds, newest first, the one that is
//! selected, and the screen's own reading of how fast the selected transfer
//! moves.
//!
//! The screen takes the shelf's place while it is open, so it has a selection
//! of its own and the detail pane follows it. Its rows come from the same poll
//! that feeds the task strip; the strip keeps a window of the newest work, the
//! screen keeps everything until `hedos pull clean` takes it.
//!
//! The rate is not in the record. The worker writes progress snapshots, and
//! the screen derives a rate from the bytes between two of them. It is the
//! screen's own reading, not a fact from disk, and dies with the screen.

use std::collections::HashMap;

use kernel::install::event::InstallProgress;
use kernel::install::pulls::PullState;
use ratatui::widgets::TableState;

use super::jobs::JobRow;

/// How much of the newest reading the smoothed rate takes. Low enough that a
/// transfer's stalls and bursts read as one figure, high enough that a change
/// of pace shows within a few polls.
const RATE_WEIGHT: f64 = 0.3;
/// How long a record may sit unchanged before its rate reading is dropped
/// rather than shown as the pace the transfer is still at. The worker writes
/// on every progress event, so a record this old is a transfer that stalled.
const STALL_MS: i64 = 5_000;

/// The screen's state.
#[derive(Debug, Default)]
pub struct PullsScreen {
    /// Every job, newest first.
    rows: Vec<JobRow>,
    /// The selection and scroll position; ratatui keeps the selected row in
    /// view through it.
    pub table: TableState,
    /// The job the selection is on, so it follows that job when rows are
    /// added above it.
    selected_job: Option<String>,
    /// The rate reading of each running transfer, by job.
    meters: HashMap<String, Meter>,
    /// The history of one job, as read on the last poll of it.
    history: Option<(String, Vec<String>)>,
    /// When the last poll read the store.
    polled_at_ms: i64,
    /// A job to put the selection on when it appears: the one a pull started
    /// from here was given, so the screen lands on what it just started.
    follow: Option<String>,
}

/// The rate of one transfer, read from consecutive records.
#[derive(Debug)]
struct Meter {
    bytes: i64,
    at_ms: i64,
    per_second: Option<f64>,
}

impl Meter {
    fn new(bytes: i64, at_ms: i64) -> Self {
        Self {
            bytes,
            at_ms,
            per_second: None,
        }
    }

    /// Take the next record; whether the reading changed. A record that has
    /// not moved on says nothing, and one that went backwards, which a
    /// restarted attempt does, starts the reading over.
    fn sample(&mut self, bytes: i64, at_ms: i64) -> bool {
        if at_ms <= self.at_ms || bytes == self.bytes {
            return false;
        }
        if bytes < self.bytes {
            *self = Self::new(bytes, at_ms);
            return true;
        }
        let instant = (bytes - self.bytes) as f64 * 1000.0 / (at_ms - self.at_ms) as f64;
        self.per_second = Some(match self.per_second {
            Some(smoothed) => smoothed + RATE_WEIGHT * (instant - smoothed),
            None => instant,
        });
        self.bytes = bytes;
        self.at_ms = at_ms;
        true
    }
}

/// The screen's reading of a transfer: bytes a second, and how long the rest
/// would take at that pace when the total is firm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub bytes_per_second: i64,
    pub left_ms: Option<i64>,
}

impl PullsScreen {
    /// The rows, newest first.
    pub fn rows(&self) -> &[JobRow] {
        &self.rows
    }

    /// The index of the selected row.
    pub fn selected(&self) -> usize {
        self.table.selected().unwrap_or(0)
    }

    /// The selected job, if there is one.
    pub fn selected_row(&self) -> Option<&JobRow> {
        self.rows.get(self.selected())
    }

    /// Take a poll of the job directory; whether anything the screen shows
    /// changed. The selection stays on its job when rows land above it, and
    /// falls to the nearest row when its job is gone.
    ///
    /// The answer is nearly always yes while the screen is open, because the
    /// ages are re-phrased on every poll; it is a signal to redraw, not a
    /// cheap diff.
    pub fn sync(&mut self, polled: &[JobRow]) -> bool {
        let mut rows = polled.to_vec();
        rows.sort_by(|left, right| {
            right
                .descriptor
                .created_at_ms
                .cmp(&left.descriptor.created_at_ms)
                .then_with(|| right.job.cmp(&left.job))
        });
        let mut changed = rows != self.rows;
        for row in &rows {
            let (bytes, at_ms) = (
                row.status.progress.bytes_downloaded,
                row.status.updated_at_ms,
            );
            match (row.pull_state, self.meters.get_mut(&row.job)) {
                (PullState::Running, Some(meter)) => changed |= meter.sample(bytes, at_ms),
                (PullState::Running, None) => {
                    self.meters
                        .insert(row.job.clone(), Meter::new(bytes, at_ms));
                }
                (_, Some(_)) => {
                    self.meters.remove(&row.job);
                    changed = true;
                }
                (_, None) => {}
            }
        }
        self.meters
            .retain(|job, _| rows.iter().any(|row| &row.job == job));
        if let Some(at) = rows.iter().map(|row| row.polled_at_ms).max() {
            self.polled_at_ms = at;
        }
        self.rows = rows;
        let followed = self
            .follow
            .as_deref()
            .and_then(|job| self.rows.iter().position(|row| row.job == job));
        if followed.is_some() {
            self.follow = None;
        }
        let index = followed
            .or_else(|| {
                let job = self.selected_job.as_deref()?;
                self.rows.iter().position(|row| row.job == job)
            })
            .unwrap_or(self.selected());
        self.select(index);
        changed
    }

    /// Move the selection by `delta` rows; whether it moved.
    pub fn step(&mut self, delta: isize) -> bool {
        let index = self.selected().saturating_add_signed(delta);
        self.select(index)
    }

    /// Put the selection on row `index`, or the last row when there are
    /// fewer; whether it moved.
    pub fn select(&mut self, index: usize) -> bool {
        let last = self.rows.len().saturating_sub(1);
        let index = index.min(last);
        let job = self.rows.get(index).map(|row| row.job.clone());
        let moved = self.table.selected() != Some(index) || self.selected_job != job;
        self.table.select(Some(index));
        self.selected_job = job;
        moved
    }

    /// Have `job` take the selection once a poll lists it.
    pub fn follow(&mut self, job: String) {
        self.follow = Some(job);
    }

    /// Put the selection on the newest pull still going, or the newest row
    /// when none is: what someone opening the screen most likely came for.
    pub fn select_newest_live(&mut self) {
        let index = self
            .rows
            .iter()
            .position(|row| row.pull_state.is_live())
            .unwrap_or(0);
        self.select(index);
    }

    /// Take the history of `job`; whether the screen shows it.
    pub fn history(&mut self, job: String, lines: Vec<String>) -> bool {
        let shown = self.selected_row().is_some_and(|row| row.job == job);
        if !shown {
            return false;
        }
        let changed = self
            .history
            .as_ref()
            .is_none_or(|(had, lines_had)| *had != job || *lines_had != lines);
        self.history = Some((job, lines));
        changed
    }

    /// The selected job's history, as far as it has been read.
    pub fn history_lines(&self) -> &[String] {
        match (&self.history, self.selected_row()) {
            (Some((job, lines)), Some(row)) if *job == row.job => lines,
            _ => &[],
        }
    }

    /// The screen's reading of `job`'s transfer, once it has two records to
    /// read between, and for as long as the record keeps moving.
    pub fn rate(&self, job: &str, progress: &InstallProgress) -> Option<Rate> {
        let meter = self.meters.get(job)?;
        let per_second = meter.per_second?;
        if per_second <= 0.0 || self.polled_at_ms.saturating_sub(meter.at_ms) > STALL_MS {
            return None;
        }
        let left_ms = match (progress.total_bytes, progress.total_is_partial) {
            (Some(total), false) => Some(
                ((total - progress.bytes_downloaded).max(0) as f64 * 1000.0 / per_second) as i64,
            ),
            _ => None,
        };
        Some(Rate {
            bytes_per_second: per_second as i64,
            left_ms,
        })
    }
}

#[cfg(test)]
mod tests;
