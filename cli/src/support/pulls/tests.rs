use super::*;

use kernel::install::pulls::{PullEvent, PullState};

use crate::support::pulls::testing::{TempDir, held, job as make_job, moved, status};

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
fn a_long_message_is_cut_to_the_width_it_is_given() {
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
