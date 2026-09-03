use super::*;

use kernel::install::pulls::{self, PullLock};

use crate::commands::pull::testing::{TempDir, job as make_job};

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

#[test]
fn pausing_a_running_pull_writes_the_ask_for_its_worker() {
    let directory = TempDir::new("pause-running");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    pause(&store, job.id(), &out()).expect("pause the pull");

    assert_eq!(job.control(), Some(PullControl::Pause));
    assert_eq!(job.status().state, PullState::Running);
}

#[test]
fn pausing_a_pull_that_is_not_running_is_refused() {
    let directory = TempDir::new("pause-stopped");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Paused);

    let error = pause(&store, job.id(), &out()).expect_err("a paused pull cannot be paused");

    assert!(error.message.contains("not running"));
    assert_eq!(job.control(), None);
}

#[test]
fn pausing_a_pull_no_worker_took_up_is_refused_rather_than_left_unread() {
    let directory = TempDir::new("pause-abandoned");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(now_millis() - 60_000, |status| {
        status.state = PullState::Queued
    })
    .expect("age the record");

    let error = pause(&store, job.id(), &out()).expect_err("nothing will read the ask");

    assert!(error.message.contains("no worker took up"));
    assert_eq!(job.control(), None);
}

#[test]
fn cancelling_a_running_pull_only_asks() {
    let directory = TempDir::new("cancel-running");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = worker_on(&job, PullState::Running);

    cancel(&store, job.id(), &out()).expect("cancel the pull");

    assert_eq!(job.control(), Some(PullControl::Cancel));
    assert_eq!(job.status().state, PullState::Running);
}

#[test]
fn cancelling_a_stopped_pull_settles_it_here() {
    let directory = TempDir::new("cancel-stopped");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Paused);

    cancel(&store, job.id(), &out()).expect("cancel the pull");

    let status = job.status();
    assert_eq!(status.state, PullState::Cancelled);
    assert_eq!(status.pid, None);
    // The ask outlives the record it settled: a worker still starting up reads
    // it and stands down instead of downloading over a cancelled job.
    assert_eq!(job.control(), Some(PullControl::Cancel));
}

#[test]
fn a_pull_that_lands_while_it_is_being_cancelled_is_not_recorded_as_cancelled() {
    let directory = TempDir::new("cancel-race");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    // The state a worker that finished in the moment between the client's read
    // and its write leaves behind: done, with the lock already released.
    let holder = worker_on(&job, PullState::Running);
    drop(holder);
    stopped(&job, PullState::Done);

    let error = cancel(&store, job.id(), &out()).expect_err("a landed pull cannot be cancelled");

    assert!(error.message.contains("already done"));
    assert_eq!(job.status().state, PullState::Done);
}

#[test]
fn cancelling_a_pull_that_already_ended_is_refused() {
    let directory = TempDir::new("cancel-done");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    stopped(&job, PullState::Done);

    let error = cancel(&store, job.id(), &out()).expect_err("a done pull cannot be cancelled");

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

    clean(&store, &CleanArgs { keep: 0 }, &out()).expect("clean the store");

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

    clean(&store, &CleanArgs { keep: 1 }, &out()).expect("clean the store");

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
