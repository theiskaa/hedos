use super::*;

use kernel::install::event::InstallProgress;
use kernel::install::pulls::START_GRACE_MS;

use crate::support::pulls::testing::{TempDir, job as make_job};

#[test]
fn a_pull_that_has_moved_bytes_reads_as_a_download() {
    let directory = TempDir::new("jobs-downloading");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = job.claim().expect("claim").expect("the lock is free");
    job.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
        status.progress = InstallProgress {
            bytes_downloaded: 512,
            total_bytes: Some(1_024),
            total_is_partial: false,
            current_file: None,
        };
    })
    .expect("write the record");

    let rows = rows(&store, 2_000);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].job, job.id());
    assert_eq!(rows[0].reference, "Qwen/Qwen3-8B");
    assert!(matches!(rows[0].state, TaskState::Downloading(_)));
}

#[test]
fn a_pull_with_nothing_to_show_yet_says_what_it_is_waiting_for() {
    let directory = TempDir::new("jobs-queued");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = job.claim().expect("claim").expect("the lock is free");
    job.update_status(1_000, |status| {
        status.state = PullState::Queued;
        status.pid = Some(std::process::id());
    })
    .expect("write the record");

    assert_eq!(
        rows(&store, 2_000)[0].state,
        TaskState::Status("queued".to_owned())
    );
}

#[test]
fn a_pull_the_user_stopped_and_one_that_was_cut_off_both_offer_to_go_on() {
    let directory = TempDir::new("jobs-stopped");
    let store = directory.store();
    for (reference, state) in [
        ("a/paused", PullState::Paused),
        ("a/interrupted", PullState::Interrupted),
    ] {
        let job = make_job(&store, reference, 1_000);
        job.update_status(1_000, |status| status.state = state)
            .expect("write the record");
    }

    let rows = rows(&store, 2_000);

    assert!(
        rows.iter()
            .all(|row| matches!(row.state, TaskState::Stopped(_)))
    );
    assert!(rows.iter().all(|row| !row.state.running()));
}

#[test]
fn a_cancelled_pull_is_an_ending_rather_than_a_failure() {
    let directory = TempDir::new("jobs-cancelled");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| status.state = PullState::Cancelled)
        .expect("write the record");

    assert_eq!(
        rows(&store, 2_000)[0].state,
        TaskState::Done("cancelled".to_owned())
    );
}

#[test]
fn a_landed_pull_names_what_it_fetched() {
    let directory = TempDir::new("jobs-done");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| status.state = PullState::Done)
        .expect("write the record");

    assert_eq!(
        rows(&store, 2_000)[0].state,
        TaskState::Done("pulled Qwen/Qwen3-8B".to_owned())
    );
}

#[test]
fn a_failed_pull_carries_the_reason_it_failed() {
    let directory = TempDir::new("jobs-failed");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| {
        status.state = PullState::Failed;
        status.message = Some("needs a token".to_owned());
    })
    .expect("write the record");

    assert_eq!(
        rows(&store, 2_000)[0].state,
        TaskState::Failed("needs a token".to_owned())
    );
}

#[test]
fn a_store_that_was_never_made_has_no_pulls_rather_than_an_error() {
    let directory = TempDir::new("jobs-empty");
    assert!(rows(&directory.store(), 2_000).is_empty());
}

#[test]
fn a_pull_that_ended_long_ago_is_not_shown_at_all() {
    // Its record stays in the store until someone runs `hedos pull clean`, so a
    // strip that took every ended job would put back every row it expired.
    let directory = TempDir::new("jobs-stale");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| status.state = PullState::Done)
        .expect("write the record");

    assert_eq!(rows(&store, 2_000).len(), 1);
    assert!(rows(&store, 1_000 + ENDED_LINGER_MS).is_empty());
}

#[test]
fn a_pull_no_worker_took_up_reads_as_stopped_rather_than_as_a_queue() {
    // The kernel already refuses to join one, so a strip that called it live
    // would leave no way to pull that model again.
    let directory = TempDir::new("jobs-abandoned");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);

    let rows = rows(&store, 1_000 + START_GRACE_MS + 1);

    assert_eq!(rows[0].pull_state, PullState::Interrupted);
    assert!(!rows[0].pull_state.is_live());
    assert!(rows[0].pull_state.is_resumable());
    assert!(matches!(rows[0].state, TaskState::Stopped(_)));
    assert_eq!(job.stored_status().state, PullState::Queued);
}
