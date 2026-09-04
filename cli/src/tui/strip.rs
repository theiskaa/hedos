//! The task strip: the rows for background work and for what ran in the
//! foreground, oldest first, each expiring a while after it finished. A
//! finished row knows the tick it finished on, and the strip decides which
//! rows a key acts on, so the reducer and the painter never disagree.

use std::collections::HashSet;

use kernel::install::event::InstallProgress;
use kernel::install::pulls::PullState;

use super::jobs::JobRow;
use super::tasks::{TaskEvent, TaskId, TaskKind, TaskLabel, TaskState};

/// How long a finished task stays in the strip, and how long a failed one
/// does, in ticks.
pub(super) const DONE_LINGER_TICKS: u64 = 60 * super::app::TICKS_PER_SECOND;
pub(super) const FAILED_LINGER_TICKS: u64 = 10 * 60 * super::app::TICKS_PER_SECOND;
/// The same window as [`FAILED_LINGER_TICKS`] in wall-clock milliseconds, for
/// deciding which ended pulls the strip takes on at all: one that ended before
/// the screen opened is not news. A row the strip has already expired is
/// remembered separately, since a done row goes sooner than this.
pub(super) const ENDED_LINGER_MS: i64 =
    (FAILED_LINGER_TICKS / super::app::TICKS_PER_SECOND) as i64 * 1_000;

/// Where a row comes from. Only a task is something this process is running: a
/// pull belongs to a worker of its own, and a hand-off has already ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowSource {
    /// A task running here, which quitting may have to wait for.
    Task(TaskKind),
    /// A pull running in a worker of its own, named by the job it writes.
    Pull(String),
    /// Something that owned the terminal while the screen stepped aside.
    HandOff,
}

/// A task as the strip shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: TaskId,
    pub label: TaskLabel,
    pub source: RowSource,
    pub state: TaskState,
    /// Where a pull's own record says it is. Finer than `state`, which is what
    /// the row is drawn from: `cancelled` and `done` both read as finished on
    /// screen, and only one of them put a model on the shelf.
    pub pull_state: Option<PullState>,
    /// What a pull has landed, whatever `state` shows; nothing for a task run
    /// here.
    pub progress: InstallProgress,
    /// The tick the task finished on, for expiry.
    finished_at: Option<u64>,
}

impl TaskRow {
    /// Whether the task is still going.
    pub fn running(&self) -> bool {
        self.state.running()
    }

    /// The kind of task this row is running, if it is running one here.
    pub fn kind(&self) -> Option<&TaskKind> {
        match &self.source {
            RowSource::Task(kind) => Some(kind),
            _ => None,
        }
    }

    /// Whether this row shows a pull that is still going, queued or
    /// downloading.
    pub fn pull_going(&self) -> bool {
        self.pull_state.is_some_and(PullState::is_live)
    }

    /// The pull job this row shows, if it shows one.
    pub fn job(&self) -> Option<&str> {
        match &self.source {
            RowSource::Pull(job) => Some(job),
            _ => None,
        }
    }
}

/// The strip's rows.
#[derive(Debug, Default)]
pub struct TaskStrip {
    rows: Vec<TaskRow>,
    /// The pull jobs whose rows have expired, so the next poll, which still
    /// lists them, does not put them back.
    expired: HashSet<String>,
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
            source: RowSource::Task(kind),
            state: TaskState::Running,
            pull_state: None,
            progress: InstallProgress::default(),
            finished_at: None,
        });
    }

    /// A row for something that already ran, in the foreground, ending on
    /// tick `now`.
    pub fn record(&mut self, label: TaskLabel, state: TaskState, now: u64) {
        self.rows.push(TaskRow {
            id: TaskId::next(),
            label,
            source: RowSource::HandOff,
            state,
            pull_state: None,
            progress: InstallProgress::default(),
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

    /// Fold the pull jobs into the strip on tick `now`; whether anything moved.
    ///
    /// A job keeps one row for its whole life, so the row a key acts on does not
    /// change under the user between polls. A job that has left the store keeps
    /// its row until it expires like any other finished one, because a record
    /// swept away is not a reason to make a finished download vanish mid-glance.
    pub fn sync_pulls(&mut self, jobs: Vec<JobRow>, now: u64) -> PullChanges {
        let mut changes = PullChanges::default();
        for job in jobs {
            match self.rows.iter_mut().find(|row| row.job() == Some(&job.job)) {
                Some(row) => {
                    // The record's own state is what a landing is read from,
                    // and it is recorded before the display line is compared: a
                    // line that happens to read the same either side of the
                    // ending must not swallow the ending itself.
                    if row.pull_state != Some(job.pull_state) {
                        row.pull_state = Some(job.pull_state);
                        changes.moved = true;
                        if job.pull_state == PullState::Done {
                            changes.landed.push(job.reference);
                        }
                    }
                    if row.progress != job.status.progress {
                        row.progress = job.status.progress;
                        changes.moved = true;
                    }
                    if row.state == job.state {
                        continue;
                    }
                    row.state = job.state;
                    row.finished_at = match row.running() {
                        true => None,
                        false => row.finished_at.or(Some(now)),
                    };
                    changes.moved = true;
                }
                None => {
                    // A pull that ended long ago is left out rather than shown
                    // and then expired: its record stays until someone cleans
                    // it, and a strip that took every ended job would put back
                    // every row it expired on the very next poll.
                    if job.aged_out || self.expired.contains(&job.job) {
                        continue;
                    }
                    let finished_at = (!job.state.running()).then_some(now);
                    self.rows.push(TaskRow {
                        id: TaskId::next(),
                        label: TaskLabel {
                            verb: "pull",
                            subject: job.reference,
                        },
                        source: RowSource::Pull(job.job),
                        state: job.state,
                        pull_state: Some(job.pull_state),
                        progress: job.status.progress,
                        finished_at,
                    });
                    changes.moved = true;
                }
            }
        }
        changes
    }

    /// Drop the rows whose time is up on tick `now`; whether any was.
    pub fn expire(&mut self, now: u64) -> bool {
        let before = self.rows.len();
        let expired = &mut self.expired;
        self.rows.retain(|row| {
            // A stopped pull is not history: it is waiting on the user, the key
            // that carries it on is on its row, and the job directory keeps it
            // as long as it takes. Ageing it off screen would hide the only
            // place the screen offers to resume it.
            if matches!(row.state, TaskState::Stopped(_)) {
                return true;
            }
            let kept = row.finished_at.is_none_or(|finished| {
                let linger = match row.state {
                    TaskState::Failed(_) => FAILED_LINGER_TICKS,
                    _ => DONE_LINGER_TICKS,
                };
                now < finished + linger
            });
            if !kept && let Some(job) = row.job() {
                expired.insert(job.to_owned());
            }
            kept
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

    /// Whether a task this process is running has still to finish.
    ///
    /// A pull is not one of them: it belongs to a worker that outlives this
    /// process, so quitting never waits for one.
    pub fn busy(&self) -> bool {
        self.rows
            .iter()
            .any(|row| row.running() && matches!(row.source, RowSource::Task(_)))
    }

    /// Whether a running task concerns model `id`.
    pub fn running_on(&self, id: &str) -> bool {
        self.rows
            .iter()
            .any(|row| row.running() && row.kind().and_then(TaskKind::model_id) == Some(id))
    }

    /// The models the pulls still going are fetching.
    pub fn pulling(&self) -> Vec<String> {
        self.going().map(|row| row.label.subject.clone()).collect()
    }

    /// Whether any pull is still going, without naming them.
    pub fn any_pulling(&self) -> bool {
        self.going().next().is_some()
    }

    /// Whether a pull of `reference` is still going.
    pub fn is_pulling(&self, reference: &str) -> bool {
        self.going().any(|row| row.label.subject == reference)
    }

    fn going(&self) -> impl Iterator<Item = &TaskRow> {
        self.rows.iter().filter(|row| row.pull_going())
    }

    /// The row showing pull `job`, whatever state it is in.
    pub fn pull_row(&self, job: &str) -> Option<&TaskRow> {
        self.rows.iter().find(|row| row.job() == Some(job))
    }

    /// Whether a task of `kind`'s shape is already running here.
    pub fn already_running(&self, kind: &TaskKind) -> bool {
        self.rows
            .iter()
            .any(|row| row.running() && row.kind() == Some(kind))
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
    pub fn shown_failure(&self, height: usize) -> Option<TaskId> {
        let shown = self.shown(height);
        self.newest_failure()
            .filter(|id| shown.iter().any(|row| row.id == *id))
    }

    /// The rows whose hints act among those a strip `height` rows tall
    /// shows; a target off screen gets no hint, and the key does not act on
    /// it either. `is_selected` says whether a pull reference names the
    /// selected record. The painter passes the height it drew, the reducer
    /// the layout's cap; the two differ only on a terminal too short for
    /// the whole strip, where the drawn height falls under the cap.
    pub fn hint_targets(&self, height: usize, is_selected: impl Fn(&str) -> bool) -> HintTargets {
        let shown = self.shown(height);
        let visible = |id: TaskId| shown.iter().any(|row| row.id == id);
        let done_on_selected = shown
            .iter()
            // Only a pull that landed put a model on the shelf; a cancelled one
            // ends the same way on screen and installed nothing.
            .filter(|row| match row.pull_state {
                Some(PullState::Done) => is_selected(&row.label.subject),
                _ => false,
            })
            .map(|row| row.id)
            .collect();
        HintTargets {
            failure: self.shown_failure(height),
            pull: self.newest_running_pull().filter(|id| visible(*id)),
            stopped: self.newest_stopped_pull().filter(|id| visible(*id)),
            done_on_selected,
        }
    }

    /// The newest pull still going, queued or downloading.
    pub fn newest_running_pull(&self) -> Option<TaskId> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.pull_going())
            .map(|row| row.id)
    }

    /// The newest pull that stopped with bytes worth going on from.
    pub fn newest_stopped_pull(&self) -> Option<TaskId> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.pull_state.is_some_and(PullState::is_resumable))
            .map(|row| row.id)
    }

    /// The row of task `id`.
    pub fn row(&self, id: TaskId) -> Option<&TaskRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    /// The job a row shows, by the row's id.
    pub fn job_of(&self, id: TaskId) -> Option<&str> {
        self.row(id)?.job()
    }
}

/// What a poll of the job directory changed.
///
/// A model that just landed is worth more than a repaint: the shelf has to be
/// re-read for it, and the screen selects it when it appears.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PullChanges {
    /// Whether any row moved, so the screen needs repainting.
    pub moved: bool,
    /// The models whose pulls finished on this poll, in the order the store
    /// listed them, so the last is the newest.
    pub landed: Vec<String>,
}

/// The rows whose hints act, among those on screen: the newest failure
/// answers `d`, the newest running pull `c`, and the done pulls whose model
/// is selected `w` and `l`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HintTargets {
    failure: Option<TaskId>,
    pull: Option<TaskId>,
    stopped: Option<TaskId>,
    done_on_selected: Vec<TaskId>,
}

/// Which of a row's hints apply to it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowHints {
    pub dismissable: bool,
    /// Whether `c` would open the stop card over the pull this row shows.
    pub stoppable: bool,
    /// Whether `R` would put a worker back on the pull this row shows.
    pub resumable: bool,
    /// Whether `w` and `l` would act on the model this row pulled.
    pub on_selected: bool,
}

impl HintTargets {
    /// The failure `d` dismisses, if one is on screen.
    pub fn failure(&self) -> Option<TaskId> {
        self.failure
    }

    /// The pull `c` offers to stop, if one is on screen.
    pub fn pull(&self) -> Option<TaskId> {
        self.pull
    }

    /// The stopped pull `R` resumes, if one is on screen.
    pub fn stopped(&self) -> Option<TaskId> {
        self.stopped
    }

    /// The hints that apply to `row`.
    pub fn for_row(&self, row: &TaskRow) -> RowHints {
        RowHints {
            dismissable: self.failure == Some(row.id),
            stoppable: self.pull == Some(row.id),
            resumable: self.stopped == Some(row.id),
            on_selected: self.done_on_selected.contains(&row.id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::testing::job_row;

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
        assert!(strip.rows()[1].kind().is_none());
    }

    /// A strip holding one pull of `reference` in `state`, as a poll of the job
    /// directory leaves it.
    fn strip_pulling(reference: &str, pull_state: PullState, state: TaskState) -> TaskStrip {
        let mut strip = TaskStrip::default();
        strip.sync_pulls(vec![job_row(reference, pull_state, state)], 0);
        strip
    }

    #[test]
    fn hints_target_only_the_rows_a_strip_shows() {
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Queued,
            TaskState::Status("queued".to_owned()),
        );
        let pull = strip.rows()[0].id;
        let targets = strip.hint_targets(4, |_| false);
        assert_eq!(targets.pull(), Some(pull));
        assert_eq!(targets.failure(), None);
        assert!(targets.for_row(&strip.rows()[0]).stoppable);
        for _ in 0..4 {
            strip.start(TaskId::next(), TaskKind::Scan);
        }
        assert_eq!(strip.hint_targets(4, |_| false).pull(), None);
        assert_eq!(strip.hint_targets(5, |_| false).pull(), Some(pull));
        strip.sync_pulls(
            vec![job_row(
                "gemma3",
                PullState::Done,
                TaskState::Done("pulled gemma3".to_owned()),
            )],
            0,
        );
        let selected = strip.hint_targets(5, |reference| reference == "gemma3");
        assert!(selected.for_row(&strip.rows()[0]).on_selected);
        let elsewhere = strip.hint_targets(5, |reference| reference == "llava");
        assert!(!elsewhere.for_row(&strip.rows()[0]).on_selected);
    }

    #[test]
    fn a_job_keeps_one_row_however_often_it_is_polled() {
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Queued,
            TaskState::Status("queued".to_owned()),
        );
        let id = strip.rows()[0].id;
        let row = job_row(
            "gemma3",
            PullState::Queued,
            TaskState::Status("queued".to_owned()),
        );

        // The same record twice is not news, and the row a key acts on must not
        // change under the user between polls.
        assert!(!strip.sync_pulls(vec![row.clone()], 1).moved);
        assert_eq!(strip.rows().len(), 1);
        assert_eq!(strip.rows()[0].id, id);

        let landed = strip.sync_pulls(
            vec![JobRow {
                pull_state: PullState::Done,
                state: TaskState::Done("pulled gemma3".to_owned()),
                ..row
            }],
            2,
        );
        assert!(landed.moved);
        assert_eq!(landed.landed, vec!["gemma3".to_owned()]);
        assert_eq!(strip.rows()[0].id, id);
    }

    #[test]
    fn a_pull_that_stopped_offers_to_go_on_and_waits_as_long_as_it_takes() {
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Paused,
            TaskState::Stopped("paused".to_owned()),
        );
        let id = strip.rows()[0].id;

        let targets = strip.hint_targets(4, |_| false);
        assert_eq!(targets.stopped(), Some(id));
        assert!(targets.for_row(&strip.rows()[0]).resumable);
        assert!(!targets.for_row(&strip.rows()[0]).stoppable);
        assert_eq!(strip.job_of(id), Some("1000-gemma3"));

        // It is waiting on the user, and the job directory keeps it for as long
        // as that takes, so ageing the row off screen would hide the only place
        // the screen offers to carry it on.
        assert!(!strip.expire(DONE_LINGER_TICKS + 1));
        assert!(!strip.expire(FAILED_LINGER_TICKS + 1));
        assert_eq!(strip.rows().len(), 1);
    }

    #[test]
    fn a_pull_already_finished_when_the_screen_opens_did_not_just_land() {
        // Opening the screen would otherwise re-select a download from days ago
        // and re-read the shelf for it.
        let mut strip = TaskStrip::default();
        let changes = strip.sync_pulls(
            vec![job_row(
                "gemma3",
                PullState::Done,
                TaskState::Done("pulled gemma3".to_owned()),
            )],
            0,
        );

        assert!(changes.moved);
        assert!(changes.landed.is_empty());
    }

    #[test]
    fn a_pull_that_lands_between_polls_is_reported_once() {
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Running,
            TaskState::Downloading(InstallProgress::default()),
        );
        let landed = job_row(
            "gemma3",
            PullState::Done,
            TaskState::Done("pulled gemma3".to_owned()),
        );

        assert_eq!(
            strip.sync_pulls(vec![landed.clone()], 1).landed,
            vec!["gemma3".to_owned()]
        );
        // The same record again is not a second landing.
        assert!(strip.sync_pulls(vec![landed], 2).landed.is_empty());
    }

    #[test]
    fn a_job_swept_from_the_store_keeps_its_row_until_it_expires() {
        // A record collected by `hedos pull clean` is not a reason for a
        // finished download to vanish from under the reader's eyes.
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Done,
            TaskState::Done("pulled gemma3".to_owned()),
        );

        assert!(!strip.sync_pulls(Vec::new(), 1).moved);
        assert_eq!(strip.rows().len(), 1);
        assert!(strip.expire(DONE_LINGER_TICKS + 1));
        assert!(strip.rows().is_empty());
    }

    #[test]
    fn a_record_that_moves_under_an_unchanged_row_is_still_followed() {
        // `queued` with nothing to say and `running` with no bytes yet both draw
        // the same line, so the display state cannot be what the row tracks.
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Queued,
            TaskState::Status("queued".to_owned()),
        );
        strip.sync_pulls(
            vec![job_row(
                "gemma3",
                PullState::Running,
                TaskState::Status("queued".to_owned()),
            )],
            1,
        );

        assert_eq!(strip.rows()[0].pull_state, Some(PullState::Running));
    }

    #[test]
    fn a_row_that_expired_is_not_put_back_by_the_next_poll() {
        // The record outlives the row, so a strip that re-added every job it
        // still saw would show last week's downloads for good.
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Done,
            TaskState::Done("pulled gemma3".to_owned()),
        );
        assert!(strip.expire(DONE_LINGER_TICKS + 1));
        assert!(strip.rows().is_empty());

        // The store still lists the job, well inside the window the strip
        // takes ended pulls on; the row it expired stays gone.
        let done = job_row(
            "gemma3",
            PullState::Done,
            TaskState::Done("pulled gemma3".to_owned()),
        );
        assert!(
            !strip
                .sync_pulls(vec![done.clone()], DONE_LINGER_TICKS + 2)
                .moved
        );
        assert!(strip.rows().is_empty());

        // A pull that ended before the screen opened is not taken on at all.
        let mut old = job_row(
            "other",
            PullState::Done,
            TaskState::Done("pulled other".to_owned()),
        );
        old.aged_out = true;
        assert!(!strip.sync_pulls(vec![old], DONE_LINGER_TICKS + 3).moved);
        assert!(strip.rows().is_empty());
    }

    #[test]
    fn a_landing_survives_a_display_line_that_did_not_move() {
        // The record's state is what a landing is read from, so a wording that
        // happened to match the line before it cannot swallow one.
        let mut strip = strip_pulling(
            "gemma3",
            PullState::Running,
            TaskState::Status("busy".to_owned()),
        );
        let changes = strip.sync_pulls(
            vec![job_row(
                "gemma3",
                PullState::Done,
                TaskState::Status("busy".to_owned()),
            )],
            1,
        );

        assert_eq!(changes.landed, vec!["gemma3".to_owned()]);
        assert!(changes.moved);
    }

    #[test]
    fn a_cancelled_pull_never_offers_to_warm_what_it_did_not_install() {
        // It ends the same way a landed pull does on screen, and installed
        // nothing, so `w` and `l` would act on a model that is not there.
        let strip = strip_pulling(
            "gemma3",
            PullState::Cancelled,
            TaskState::Done("cancelled".to_owned()),
        );
        let targets = strip.hint_targets(10, |reference| reference == "gemma3");

        assert!(!targets.for_row(&strip.rows()[0]).on_selected);

        let landed = strip_pulling(
            "gemma3",
            PullState::Done,
            TaskState::Done("pulled gemma3".to_owned()),
        );
        let targets = landed.hint_targets(10, |reference| reference == "gemma3");
        assert!(targets.for_row(&landed.rows()[0]).on_selected);
    }

    #[test]
    fn a_pull_that_ended_answers_neither_stop_nor_resume() {
        for (place, state) in [
            (PullState::Done, TaskState::Done("pulled gemma3".to_owned())),
            (
                PullState::Cancelled,
                TaskState::Done("cancelled".to_owned()),
            ),
            (PullState::Failed, TaskState::Failed("no".to_owned())),
        ] {
            let strip = strip_pulling("gemma3", place, state);
            let targets = strip.hint_targets(10, |_| false);
            assert_eq!(targets.pull(), None, "{place} should not answer c");
            assert_eq!(targets.stopped(), None, "{place} should not answer R");
        }
    }

    #[test]
    fn a_pull_is_not_a_task_this_process_has_to_finish() {
        let strip = strip_pulling(
            "gemma3",
            PullState::Running,
            TaskState::Downloading(InstallProgress::default()),
        );
        // Quitting waits on `busy`, and a download belongs to a worker that
        // outlives this process, so waiting for one would never end well.
        assert!(!strip.busy());
        assert!(strip.any_pulling());
        assert!(strip.is_pulling("gemma3"));
        assert_eq!(strip.pulling(), vec!["gemma3".to_owned()]);
        assert!(strip.rows()[0].kind().is_none());
    }
}
