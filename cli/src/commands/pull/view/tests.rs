use super::*;

use kernel::install::event::InstallProgress;
use kernel::install::pulls::{PullEvent, PullState};

use crate::commands::pull::testing::{TempDir, job as make_job};

/// A job with a worker holding it, so the record under test is the only thing
/// the cells are reading.
struct Held {
    _directory: TempDir,
    job: PullJobDir,
    _lock: kernel::install::pulls::PullLock,
}

fn held(label: &str) -> Held {
    let directory = TempDir::new(label);
    let job = make_job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
    let lock = job
        .claim()
        .expect("claim the job")
        .expect("the lock is free");
    Held {
        _directory: directory,
        job,
        _lock: lock,
    }
}

fn status(state: PullState) -> PullStatus {
    let mut status = PullStatus::queued(1_000);
    status.state = state;
    status
}

fn moved(downloaded: i64, total: Option<i64>, partial: bool) -> PullStatus {
    let mut status = status(PullState::Running);
    status.progress = InstallProgress {
        bytes_downloaded: downloaded,
        total_bytes: total,
        total_is_partial: partial,
        current_file: None,
    };
    status
}

#[test]
fn progress_reads_as_a_percentage_against_a_firm_total() {
    let gib = 1 << 30;
    assert_eq!(
        progress(&moved(3 * gib / 2, Some(6 * gib), false)),
        "25%  1.5 GB of 6 GB"
    );
}

#[test]
fn an_estimated_total_leaves_the_bytes_to_speak_for_themselves() {
    assert_eq!(progress(&moved(3 << 20, Some(9 << 20), true)), "3 MB");
}

#[test]
fn a_pull_that_has_not_moved_shows_a_dash() {
    assert_eq!(progress(&status(PullState::Queued)), DASH);
}

#[test]
fn a_waiting_retry_is_the_note_worth_showing() {
    let held = held("note-retry");
    let mut status = status(PullState::Queued);
    status.next_attempt_at_ms = Some(10_000 + 45_000);
    status.message = Some("connection reset".to_owned());
    assert_eq!(note(&held.job, &status, 10_000), "retry in 45s");
}

#[test]
fn a_message_outranks_the_providers_last_line() {
    let held = held("note-message");
    let mut status = status(PullState::Failed);
    status.message = Some("needs a token".to_owned());
    status.status_line = Some("pulling manifest".to_owned());
    assert_eq!(note(&held.job, &status, 10_000), "needs a token");
}

#[test]
fn what_a_provider_was_doing_stops_mattering_once_the_pull_has_stopped() {
    let held = held("note-stale");
    for state in [PullState::Done, PullState::Paused, PullState::Cancelled] {
        let mut stopped = status(state);
        stopped.status_line = Some("Resolving Qwen/Qwen3-8B".to_owned());
        assert_eq!(note(&held.job, &stopped, 10_000), "");
    }

    let mut running = status(PullState::Running);
    running.status_line = Some("Resolving Qwen/Qwen3-8B".to_owned());
    assert_eq!(note(&held.job, &running, 10_000), "Resolving Qwen/Qwen3-8B");
}

#[test]
fn an_attempt_past_the_first_is_worth_saying_on_its_own() {
    let held = held("note-attempt");
    let mut retried = status(PullState::Running);
    retried.attempt = 3;
    assert_eq!(note(&held.job, &retried, 10_000), "attempt 3");
    assert_eq!(note(&held.job, &status(PullState::Running), 10_000), "");
}

#[test]
fn a_pull_no_worker_took_up_says_so_rather_than_reading_as_a_queue() {
    // Waiting behind a busy slot and waiting for a worker that is not coming
    // are the same record; only one of them is going to move.
    let directory = TempDir::new("note-abandoned");
    let job = make_job(&directory.store(), "Qwen/Qwen3-8B", 1_000);

    assert_eq!(note(&job, &status(PullState::Queued), 1_000), "");
    assert_eq!(note(&job, &status(PullState::Queued), 99_000), "no worker");
}

#[test]
fn a_long_message_is_cut_to_the_width_of_the_column() {
    let held = held("note-long");
    let mut status = status(PullState::Failed);
    status.message = Some("x".repeat(200));
    let note = note(&held.job, &status, 0);
    assert_eq!(note.chars().count(), NOTE_LIMIT);
    assert!(note.ends_with('…'));
}

#[test]
fn history_lines_say_how_long_ago_and_what() {
    let state = PullEvent {
        at_ms: 60_000,
        kind: PullEventKind::State {
            state: PullState::Running,
        },
    };
    assert_eq!(event_line(&state, 180_000), "   2m ago  running");

    let retry = PullEvent {
        at_ms: 170_000,
        kind: PullEventKind::Retry {
            attempt: 2,
            reason: "connection reset".to_owned(),
            delay_ms: 15_000,
        },
    };
    assert_eq!(
        event_line(&retry, 180_000),
        "  10s ago  attempt 2 failed, retrying in 15s: connection reset"
    );
}

#[test]
fn the_table_lines_its_columns_up_under_a_header() {
    let held = held("view-table");
    let table = table(
        &[(held.job.clone(), moved(1 << 30, Some(4 << 30), false))],
        2_000,
    );

    let mut lines = table.lines();
    let header = lines.next().expect("a header row");
    let row = lines.next().expect("one job row");
    assert!(row.contains("Qwen/Qwen3-8B"));
    assert!(row.contains("running"));
    assert!(row.contains("25%"));
    let reference_column = header.find("REFERENCE").expect("a reference column");
    assert_eq!(row.find("Qwen/Qwen3-8B"), Some(reference_column));
}

#[test]
fn detaching_names_the_commands_that_reach_the_download_again() {
    let held = held("view-detached");
    let said = detached(&held.job);

    assert!(said.contains("Qwen/Qwen3-8B"));
    assert!(said.contains(&format!("hedos pull attach {}", held.job.id())));
    assert!(said.contains(&format!("hedos pull cancel {}", held.job.id())));
}

#[test]
fn a_stopped_pull_is_told_how_to_go_on() {
    let held = held("view-resumable");
    let mut status = status(PullState::Interrupted);
    status.message = Some("connection reset".to_owned());
    let said = resumable(&held.job, &status);

    assert!(said.starts_with("interrupted: connection reset"));
    assert!(said.contains(&format!("hedos pull resume {}", held.job.id())));
}

#[test]
fn json_carries_the_descriptor_and_the_record_in_one_object() {
    let held = held("view-json");
    let value = json(&held.job, &moved(64, Some(128), false));

    assert_eq!(value["reference"], "Qwen/Qwen3-8B");
    assert_eq!(value["provider"], "huggingface");
    assert_eq!(value["state"], "running");
    assert_eq!(value["progress"]["bytes_downloaded"], 64);
    assert_eq!(value["id"], held.job.id());
}
