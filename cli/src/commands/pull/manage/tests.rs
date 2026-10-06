use super::*;

use std::time::Duration;

use kernel::install::pulls::{self, PullEventKind, PullLock, PullState};

use crate::support::pulls::testing::{TempDir, job as make_job, mark_registering};

fn out() -> Out {
    Out::new(false)
}

/// Hold the job's lock the way a worker does, and record the pid a worker
/// writes, so the record reads as a live pull.
fn worker_on(job: &PullJobDir, state: PullState) -> PullLock {
    let lock = pulls::take_lock(&job.lock_path())
        .expect("take the lock")
        .expect("the lock is free");
    job.update_status(now_millis(), |status| {
        status.state = state;
        status.pid = Some(std::process::id());
    })
    .expect("write the record");
    lock
}

fn stopped(job: &PullJobDir, state: PullState) {
    job.update_status(now_millis(), |status| status.state = state)
        .expect("write the record");
}

#[tokio::test]
async fn pausing_a_running_pull_writes_the_ask_for_its_worker() {
    let directory = TempDir::new("pause-running");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("pause the pull");

    assert_eq!(job.control(), Some(PullControl::Pause));
    assert_eq!(job.status().state, PullState::Running);
}

#[tokio::test]
async fn pausing_a_pull_being_registered_is_refused_rather_than_promised() {
    let directory = TempDir::new("pause-registering");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);
    // The worker has marked itself scanning the store, which it does without
    // reading the control file, so an ask reaches nobody. The byte count is
    // left alone: the mark is the whole of the signal.
    job.update_status(now_millis(), mark_registering)
        .expect("write the record");

    let error = pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("nothing is left to pause");

    assert!(error.message.contains("every byte has landed"), "{error:?}");
    assert_eq!(job.control(), None);
}

#[tokio::test]
async fn a_full_byte_count_alone_does_not_put_a_pull_past_stopping() {
    let directory = TempDir::new("pause-full-bar");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);
    // The transfer is over but the worker has not said so: it is still reading
    // the control file, so the ask is written for it. The figures alone are not
    // to be trusted here, being a previous attempt's or a partial sum.
    job.update_status(now_millis(), |status| {
        status.progress.total_bytes = Some(400);
        status.progress.bytes_downloaded = 400;
    })
    .expect("write the record");

    pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("the worker is still listening");

    assert_eq!(job.control(), Some(PullControl::Pause));
}

#[tokio::test]
async fn a_pause_that_finds_the_pull_landed_says_so_rather_than_pausing() {
    let directory = TempDir::new("pause-landed");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let lock = worker_on(&job, PullState::Running);
    // A worker that took its last byte before it next read the control file:
    // it goes on to done and lets go of the job.
    let worker = job.clone();
    let landing = std::thread::spawn(move || {
        while worker.control().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        worker
            .update_status(now_millis(), |status| {
                status.state = PullState::Done;
                status.pid = None;
            })
            .expect("settle the record");
        drop(lock);
    });

    let error = pause_within(&store, job.id(), &out(), Duration::from_secs(5))
        .await
        .expect_err("the pull landed before the pause was read");
    landing.join().expect("the worker thread");

    assert!(error.message.starts_with(job.id()), "{error:?}");
    assert!(
        error
            .message
            .contains("every byte landed before the pause was read"),
        "{error:?}"
    );
    assert_eq!(job.status().state, PullState::Done);
}

#[tokio::test]
async fn pausing_a_pull_that_is_not_running_is_refused() {
    let directory = TempDir::new("pause-stopped");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Paused);

    let error = pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("a paused pull cannot be paused");

    assert!(error.message.contains("not running"));
    assert_eq!(job.control(), None);
}

#[tokio::test]
async fn pausing_a_pull_no_worker_took_up_is_refused_rather_than_left_unread() {
    let directory = TempDir::new("pause-abandoned");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(now_millis() - 60_000, |status| {
        status.state = PullState::Queued
    })
    .expect("age the record");

    let error = pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("nothing will read the ask");

    assert!(error.message.contains("no worker took up"));
    assert_eq!(job.control(), None);
}

#[tokio::test]
async fn cancelling_a_running_pull_only_asks() {
    let directory = TempDir::new("cancel-running");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("cancel the pull");

    assert_eq!(job.control(), Some(PullControl::Cancel));
    assert_eq!(job.status().state, PullState::Running);
}

#[tokio::test]
async fn cancelling_a_stopped_pull_settles_it_here() {
    let directory = TempDir::new("cancel-stopped");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Paused);

    cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("cancel the pull");

    let status = job.status();
    assert_eq!(status.state, PullState::Cancelled);
    assert_eq!(status.pid, None);
    // Nobody is coming for a paused pull, so the ask goes with the record it
    // settled.
    assert_eq!(job.control(), None);
}

#[tokio::test]
async fn cancelling_a_pull_a_worker_may_still_be_starting_on_keeps_the_ask() {
    let directory = TempDir::new("cancel-starting");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", now_millis());

    cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("cancel the pull");

    assert_eq!(job.status().state, PullState::Cancelled);
    // Queued with no pid is how a spawned worker finds the job before it
    // holds it; the ask is what makes it stand down instead of downloading.
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[tokio::test]
async fn a_pull_that_lands_while_it_is_being_cancelled_is_not_recorded_as_cancelled() {
    let directory = TempDir::new("cancel-race");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    // The state a worker that finished in the moment between the client's read
    // and its write leaves behind: done, with the lock already released.
    let holder = worker_on(&job, PullState::Running);
    drop(holder);
    stopped(&job, PullState::Done);

    let error = cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("a landed pull cannot be cancelled");

    assert!(error.message.contains("already done"));
    assert_eq!(job.status().state, PullState::Done);
}

#[tokio::test]
async fn a_cancel_written_as_the_worker_stopped_its_own_way_is_settled_rather_than_lost() {
    let directory = TempDir::new("cancel-behind-honoured-pause");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let worker = worker_on(&job, PullState::Running);
    // The worker has already cleared the pause it is honouring, so the cancel
    // is written after its last read: it settles paused, then lets go.
    let settling = job.clone();
    let honouring = tokio::spawn(async move {
        while settling.control() != Some(PullControl::Cancel) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        stopped(&settling, PullState::Paused);
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(worker);
    });

    cancel_within(&store, job.id(), &out(), Duration::from_secs(5))
        .await
        .expect("the cancel is settled once the worker has gone");
    honouring.await.expect("the worker's side");

    assert_eq!(job.status().state, PullState::Cancelled);
}

#[tokio::test]
async fn a_pause_behind_a_waiting_cancel_is_refused_and_the_cancel_stands() {
    let directory = TempDir::new("pause-behind-cancel");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect("the cancel is asked");
    let error = pause_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("a pause would change nothing");

    assert!(
        error.message.contains("a cancel is already waiting"),
        "{error:?}"
    );
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn the_json_outcome_tells_a_stop_that_took_from_one_too_late_or_still_pending() {
    assert_eq!(
        outcome(Some(StopAnswer::Honoured(PullState::Paused))),
        "honoured"
    );
    assert_eq!(outcome(Some(StopAnswer::Landed)), "too_late");
    assert_eq!(
        outcome(Some(StopAnswer::Ended(PullState::Failed))),
        "too_late"
    );
    assert_eq!(outcome(None), "pending");

    let job = serde_json::json!({ "state": "running" });
    assert_eq!(view::with_outcome(job, "pending")["outcome"], "pending");
}

#[tokio::test]
async fn cancelling_a_pull_that_already_ended_is_refused() {
    let directory = TempDir::new("cancel-done");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Done);

    let error = cancel_within(&store, job.id(), &out(), Duration::ZERO)
        .await
        .expect_err("a done pull cannot be cancelled");

    assert!(error.message.contains("already done"));
    assert_eq!(job.control(), None);
}

#[test]
fn resuming_everything_with_nothing_stopped_says_so() {
    let directory = TempDir::new("resume-none");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Done);

    let args = ResumeArgs {
        job: None,
        all: true,
    };
    let error = resume(&store, &args, &out()).expect_err("nothing to resume");

    assert!(error.message.contains("no stopped pulls"));
}

#[test]
fn resuming_everything_reports_a_store_it_cannot_read() {
    let directory = TempDir::new("resume-unreadable-store");
    let store = directory.store();
    std::fs::write(store.root(), b"not a directory").expect("block the store");

    let args = ResumeArgs {
        job: None,
        all: true,
    };
    let error = resume(&store, &args, &out()).expect_err("the store could not be read");

    assert!(!error.message.contains("no stopped pulls"), "{error:?}");
}

// A runtime, because the restart spawns its worker through one. The worker is
// this test binary, which exits at once on a flag it does not know.
#[tokio::test]
async fn resuming_everything_takes_up_a_pull_no_worker_took_up() {
    let directory = TempDir::new("resume-abandoned");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);

    let args = ResumeArgs {
        job: None,
        all: true,
    };
    resume(&store, &args, &out()).expect("the pull nobody took up is resumed");

    let record = job.stored_status();
    assert_ne!(
        record.updated_at_ms, 1_000,
        "the restart rewrote the record"
    );
    assert_eq!(record.pid, None);
    assert!(
        matches!(record.state, PullState::Queued | PullState::Failed),
        "{record:?}"
    );
}

#[test]
fn resuming_everything_leaves_an_abandoned_pull_the_user_pulled_past() {
    let directory = TempDir::new("resume-pulled-past");
    let store = directory.store();
    let abandoned = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let again = make_job(&store, "Qwen/Qwen3-8B", 2_000);
    let worker = worker_on(&again, PullState::Running);

    let args = ResumeArgs {
        job: None,
        all: true,
    };
    let error = resume(&store, &args, &out()).expect_err("nothing to resume");

    assert!(error.message.contains("no stopped pulls"), "{error:?}");
    // Written down as it read, so the pull that superseded it ending some
    // other way does not hand it back to the next resume.
    let record = abandoned.stored_status();
    assert_eq!(record.state, PullState::Failed);
    assert_eq!(record.message.as_deref(), Some(pulls::PULLED_AGAIN_LINE));
    again
        .update_status(now_millis(), |status| status.state = PullState::Failed)
        .expect("write the record");
    drop(worker);
    let error = resume(&store, &args, &out()).expect_err("still nothing to resume");
    assert!(error.message.contains("no stopped pulls"), "{error:?}");
    assert_eq!(
        abandoned.reading(now_millis()).status.state,
        PullState::Failed
    );
}

#[test]
fn resuming_by_name_without_one_asks_for_a_name_or_for_all_of_them() {
    let directory = TempDir::new("resume-unnamed");
    let store = directory.store();

    let args = ResumeArgs {
        job: None,
        all: false,
    };
    let error = resume(&store, &args, &out()).expect_err("no job named");

    assert!(error.message.contains("name a pull"));
    assert!(error.message.contains("--all"));
}

#[test]
fn a_refused_resume_is_reported_under_the_job_it_refused() {
    let directory = TempDir::new("resume-refused");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    let args = ResumeArgs {
        job: Some(job.id().to_owned()),
        all: false,
    };
    let error = resume(&store, &args, &out()).expect_err("a running pull cannot be resumed");

    // The reason alone ("already running") names nothing; a user resuming
    // several needs to know which one it was about.
    assert!(error.message.starts_with(job.id()));
    assert!(error.message.contains("already running"));
}

#[test]
fn cleaning_drops_the_ended_pulls_and_keeps_the_rest() {
    let directory = TempDir::new("clean");
    let store = directory.store();
    let done = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&done, PullState::Done);
    let paused = make_job(&store, "Qwen/Qwen3-4B", 2_000);
    stopped(&paused, PullState::Paused);

    clean(&store, 0, &out()).expect("clean the store");

    let left: Vec<String> = store
        .jobs()
        .expect("read the store")
        .iter()
        .map(|job| job.id().to_owned())
        .collect();
    assert_eq!(left, vec![paused.id().to_owned()]);
}

#[test]
fn cleaning_keeps_the_newest_ended_pulls_when_asked() {
    let directory = TempDir::new("clean-keep");
    let store = directory.store();
    for (reference, at) in [("a/one", 1_000), ("a/two", 2_000)] {
        let job = make_job(&store, reference, at);
        job.update_status(at, |status| status.state = PullState::Done)
            .expect("write the record");
    }

    clean(&store, 1, &out()).expect("clean the store");

    let left = store.jobs().expect("read the store");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].job().reference, "a/two");
}

#[test]
fn logs_of_a_pull_with_no_history_are_not_an_error() {
    let directory = TempDir::new("logs-empty");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);

    let args = LogsArgs {
        job: job.id().to_owned(),
        lines: None,
    };
    logs(&store, &args, &out()).expect("print an empty history");
}

#[test]
fn logs_show_only_the_last_lines_asked_for() {
    let directory = TempDir::new("logs-tail");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    for state in [PullState::Queued, PullState::Running, PullState::Done] {
        job.append(PullEventKind::State { state }, 1_000)
            .expect("append the event");
    }
    let events = job.events();

    assert_eq!(tail(&events, Some(2)).len(), 2);
    assert_eq!(tail(&events, Some(99)).len(), 3);
    assert_eq!(tail(&events, None).len(), 3);
    // Asking for none is not the same as having none, and neither is an error.
    assert_eq!(tail(&events, Some(0)).len(), 0);
    let args = LogsArgs {
        job: job.id().to_owned(),
        lines: Some(0),
    };
    logs(&store, &args, &out()).expect("print nothing without complaining");
}

#[test]
fn a_pull_cleaned_away_beside_one_it_read_superseded_by_is_never_resumed() {
    let directory = TempDir::new("clean-then-shelf");
    let store = directory.store();
    let abandoned = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let landed = make_job(&store, "Qwen/Qwen3-8B", 2_000);
    landed
        .update_status(2_500, |status| status.state = PullState::Done)
        .expect("write the record");

    clean(&store, 0, &out()).expect("clean the store");
    // What opening the shelf does next.
    runtime::install::collect_ended(&store, &runtime::settings::PullSettings::default());
    let resumed = runtime::install::resume_all(&store);

    assert!(resumed.is_empty(), "{resumed:?}");
    assert!(!landed.path().exists());
    assert!(
        !abandoned.path().exists(),
        "it read ended when the store was cleaned, so it went with the rest"
    );
}
