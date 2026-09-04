//! The stop card: what stopping a pull costs each way, asked before a key does
//! anything to it. Shared by every surface that stops a pull, so the strip's
//! `c` and the pulls screen's open the same card.
//!
//! A cancel is not a stop: the provider tidies the half-download away, so a
//! mis-keyed cancel throws away whatever had landed. The card offers pause
//! first, because it keeps the bytes, and cancel second, because it does not.
//! Cancel answers to `x`, the shelf's own destroying key, and not to `c`: the
//! key that opened the card must not also confirm it, or a held key would
//! cancel through the card on its repeat.

use kernel::install::event::InstallProgress;

use super::event::Key;
use super::strip::TaskRow;
use super::tasks::PullAction;

/// The pull the card is about, and what it has landed so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopCard {
    /// The job a stop is addressed to.
    pub job: String,
    /// The model being fetched.
    pub reference: String,
    /// Bytes landed so far, and the total when it is known.
    pub progress: InstallProgress,
}

/// What a key on the card asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopChoice {
    /// Stop the pull one of the two ways.
    Stop(PullAction),
    /// Leave it going.
    Keep,
}

impl StopCard {
    /// The card over the pull `row` shows; `None` for a row that is not a
    /// pull still going.
    pub fn over(row: &TaskRow) -> Option<Self> {
        let job = row.job()?;
        if !row.pull_going() {
            return None;
        }
        Some(Self {
            job: job.to_owned(),
            reference: row.label.subject.clone(),
            progress: row.progress.clone(),
        })
    }

    /// Take the figures from `row` again, and say whether the pull is still
    /// going; `None` is a pull that is no longer on the strip.
    pub fn follow(&mut self, row: Option<&TaskRow>) -> bool {
        let Some(row) = row.filter(|row| row.pull_going()) else {
            return false;
        };
        self.progress = row.progress.clone();
        true
    }

    /// What `key` asks; `None` for a key the card ignores.
    pub fn choice(key: Key) -> Option<StopChoice> {
        match key {
            Key::Char('p') => Some(StopChoice::Stop(PullAction::Pause)),
            Key::Char('x') => Some(StopChoice::Stop(PullAction::Cancel)),
            Key::Escape | Key::Char('n') => Some(StopChoice::Keep),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use kernel::install::pulls::PullState;

    use crate::tui::jobs::JobRow;
    use crate::tui::strip::TaskStrip;
    use crate::tui::tasks::{TaskId, TaskKind, TaskState};
    use crate::tui::testing::{downloading, job_row};

    fn strip_with(rows: Vec<JobRow>) -> TaskStrip {
        let mut strip = TaskStrip::default();
        strip.sync_pulls(rows, 0);
        strip
    }

    #[test]
    fn the_card_is_over_a_pull_still_going_and_nothing_else() {
        let strip = strip_with(vec![
            job_row(
                "q",
                PullState::Queued,
                TaskState::Status("queued".to_owned()),
            ),
            job_row(
                "s",
                PullState::Paused,
                TaskState::Stopped("paused".to_owned()),
            ),
        ]);
        let queued = StopCard::over(&strip.rows()[0]).expect("queued is going");
        assert_eq!(queued.job, "1000-q");
        assert_eq!(queued.reference, "q");
        assert_eq!(queued.progress, InstallProgress::default());
        assert!(StopCard::over(&strip.rows()[1]).is_none());

        // A pull queued for a retry keeps its bytes, and the card must say so
        // whatever its row reads: those are what a cancel would throw away.
        let mut retrying = job_row(
            "r",
            PullState::Queued,
            TaskState::Status("retrying in 4s".to_owned()),
        );
        retrying.progress.bytes_downloaded = 1 << 30;
        let strip = strip_with(vec![retrying]);
        let card = StopCard::over(&strip.rows()[0]).expect("a retry is going");
        assert_eq!(card.progress.bytes_downloaded, 1 << 30);

        let mut strip = TaskStrip::default();
        strip.start(TaskId::next(), TaskKind::Scan);
        assert!(StopCard::over(&strip.rows()[0]).is_none());
    }

    #[test]
    fn following_takes_the_new_figures_until_the_pull_stops() {
        let strip = strip_with(vec![downloading("x")]);
        let mut card = StopCard::over(&strip.rows()[0]).expect("downloading is going");
        let mut moved = downloading("x");
        moved.progress = InstallProgress {
            bytes_downloaded: 7,
            total_bytes: Some(9),
            ..InstallProgress::default()
        };
        moved.state = TaskState::Downloading(moved.progress.clone());
        let strip = strip_with(vec![moved]);
        assert!(card.follow(strip.rows().first()));
        assert_eq!(card.progress.bytes_downloaded, 7);

        let strip = strip_with(vec![job_row(
            "x",
            PullState::Done,
            TaskState::Done("pulled x".to_owned()),
        )]);
        assert!(!card.follow(strip.rows().first()));
        assert!(!card.follow(None));
        assert_eq!(card.progress.bytes_downloaded, 7);
    }

    #[test]
    fn cancel_never_answers_to_the_key_that_opened_the_card() {
        assert_eq!(
            StopCard::choice(Key::Char('p')),
            Some(StopChoice::Stop(PullAction::Pause))
        );
        assert_eq!(
            StopCard::choice(Key::Char('x')),
            Some(StopChoice::Stop(PullAction::Cancel))
        );
        assert_eq!(StopCard::choice(Key::Char('c')), None);
        assert_eq!(StopCard::choice(Key::Escape), Some(StopChoice::Keep));
        assert_eq!(StopCard::choice(Key::Char('n')), Some(StopChoice::Keep));
        assert_eq!(StopCard::choice(Key::Char('y')), None);
        assert_eq!(StopCard::choice(Key::Enter), None);
    }
}
