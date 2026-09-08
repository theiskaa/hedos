use super::*;

use kernel::install::event::InstallProgress;
use kernel::install::pulls::{REGISTERING_LINE, START_GRACE_MS};

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

    // Every byte landed but the worker has not said what it is doing yet: the
    // transfer is over, an ask would still reach it, and the row keeps its bar.
    job.update_status(1_500, |status| status.progress.bytes_downloaded = 1_024)
        .expect("write the record");
    let full = super::rows(&store, 2_000);
    assert!(matches!(full[0].state, TaskState::Downloading(_)));

    // Registering: that is what the row says rather than a full bar, and the
    // strip drops the stop key with it.
    job.update_status(1_600, |status| {
        status.status_line = Some(REGISTERING_LINE.to_owned())
    })
    .expect("write the record");
    let registering = super::rows(&store, 2_000);
    assert_eq!(
        registering[0].state,
        TaskState::Status(REGISTERING_LINE.to_owned())
    );
}

#[test]
fn a_provider_that_only_estimates_its_total_still_reads_as_registering() {
    // An Ollama pull never reports a full fraction, because its total is an
    // estimate. The row is read from what the worker said, not from the bytes,
    // so it says so anyway.
    let directory = TempDir::new("jobs-partial-total");
    let store = directory.store();
    let job = make_job(&store, "gemma3:4b", 1_000);
    let _worker = job.claim().expect("claim").expect("the lock is free");
    job.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
        status.progress = InstallProgress {
            bytes_downloaded: 3_000,
            total_bytes: Some(3_000),
            total_is_partial: true,
            current_file: None,
        };
        status.status_line = Some(REGISTERING_LINE.to_owned());
    })
    .expect("write the record");

    let rows = rows(&store, 2_000);
    assert_eq!(
        rows[0].state,
        TaskState::Status(REGISTERING_LINE.to_owned())
    );
    assert!(rows[0].status.past_stopping(), "and it is past stopping");
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
fn a_stopped_pull_keeps_its_figures_on_its_row() {
    let directory = TempDir::new("jobs-stopped-figures");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| {
        status.state = PullState::Paused;
        status.progress = InstallProgress {
            bytes_downloaded: 125_000_000,
            total_bytes: Some(468_000_000),
            total_is_partial: false,
            current_file: None,
        };
    })
    .expect("write the record");
    let rows = rows(&store, 2_000);
    assert_eq!(
        rows[0].state,
        TaskState::Stopped("paused · 125 MB of 468 MB".to_owned())
    );

    // An estimate only: the bytes speak for themselves; a reason follows.
    job.update_status(1_100, |status| {
        status.state = PullState::Interrupted;
        status.progress.total_is_partial = true;
        status.message = Some("the network is gone".to_owned());
    })
    .expect("write the record");
    let rows = super::rows(&store, 2_000);
    assert_eq!(
        rows[0].state,
        TaskState::Stopped("interrupted · 125 MB so far, the network is gone".to_owned())
    );
}

#[test]
fn a_failed_row_does_not_name_the_model_twice_or_cut_its_reason() {
    let directory = TempDir::new("jobs-failed-reason");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let reason = "Qwen/Qwen3-8B is gated. Accept its terms and request access at https://huggingface.co/Qwen/Qwen3-8B, then set HF_TOKEN.";
    job.update_status(1_000, |status| {
        status.state = PullState::Failed;
        status.message = Some(reason.to_owned());
    })
    .expect("write the record");
    let rows = rows(&store, 2_000);
    assert_eq!(
        rows[0].state,
        TaskState::Failed(reason["Qwen/Qwen3-8B ".len()..].to_owned())
    );
    assert_eq!(rows[0].note, reason, "the note keeps the whole of it");
}

#[test]
fn the_subject_comes_off_a_reason_only_as_a_whole_word() {
    let strip = |note: &str| without_subject(note.to_owned(), "org/Model");
    assert_eq!(strip("org/Model is gated"), "is gated");
    assert_eq!(strip("org/Model: not found"), "not found");
    assert_eq!(strip("org/Model"), "");
    assert_eq!(strip("org/Model-8B is gated"), "org/Model-8B is gated");
    assert_eq!(strip("no worker"), "no worker");
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
fn a_pull_that_ended_long_ago_is_marked_for_the_strip_to_leave_out() {
    // Its record stays in the store until someone runs `hedos pull clean`, so a
    // strip that took every ended job would put back every row it expired;
    // the pulls screen still lists it, which is where it is cleaned from.
    let directory = TempDir::new("jobs-stale");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| status.state = PullState::Done)
        .expect("write the record");

    let fresh = rows(&store, 2_000);
    assert_eq!(fresh.len(), 1);
    assert!(!fresh[0].aged_out);
    assert_eq!(fresh[0].started_ago, "1s");
    assert_eq!(fresh[0].updated_ago, "1s");
    let old = rows(&store, 1_000 + ENDED_LINGER_MS);
    assert_eq!(old.len(), 1);
    assert!(old[0].aged_out);
    assert_eq!(old[0].descriptor.reference, "Qwen/Qwen3-8B");
    assert_eq!(old[0].status.state, PullState::Done);
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
