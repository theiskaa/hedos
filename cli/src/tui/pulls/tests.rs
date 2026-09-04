use super::*;

use kernel::install::event::InstallProgress;

use crate::tui::tasks::TaskState;
use crate::tui::testing::{downloading, job_row};

/// A running pull of `reference` with `bytes` landed as of `at_ms`.
fn moving(reference: &str, bytes: i64, at_ms: i64) -> JobRow {
    let mut row = downloading(reference);
    row.status.progress = InstallProgress {
        bytes_downloaded: bytes,
        total_bytes: Some(1_000_000),
        ..InstallProgress::default()
    };
    row.status.updated_at_ms = at_ms;
    row.polled_at_ms = at_ms;
    row.state = TaskState::Downloading(row.status.progress.clone());
    row
}

/// `row` as a later poll at `polled_at_ms` reads it, the record unchanged.
fn polled_again(mut row: JobRow, polled_at_ms: i64) -> JobRow {
    row.polled_at_ms = polled_at_ms;
    row
}

fn created(mut row: JobRow, at_ms: i64) -> JobRow {
    row.descriptor.created_at_ms = at_ms;
    row
}

#[test]
fn rows_are_listed_newest_first_and_the_selection_follows_its_job() {
    let mut screen = PullsScreen::default();
    assert!(screen.sync(&[
        created(downloading("old"), 1),
        created(downloading("new"), 2)
    ]));
    assert_eq!(screen.rows()[0].reference, "new");
    assert!(screen.step(1));
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("old")
    );

    // A newer job lands above: the selection stays on `old`.
    assert!(screen.sync(&[
        created(downloading("old"), 1),
        created(downloading("new"), 2),
        created(downloading("newest"), 3),
    ]));
    assert_eq!(screen.selected(), 2);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("old")
    );

    // Its job is cleaned away: the selection falls to the nearest row.
    assert!(screen.sync(&[
        created(downloading("new"), 2),
        created(downloading("newest"), 3)
    ]));
    assert_eq!(screen.selected(), 1);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("new")
    );
    assert!(!screen.step(1));
    assert!(screen.step(-1));
    assert!(!screen.step(-1));
    assert!(!screen.sync(&[
        created(downloading("new"), 2),
        created(downloading("newest"), 3)
    ]));
}

#[test]
fn a_pull_started_here_takes_the_selection_when_it_appears() {
    let mut screen = PullsScreen::default();
    screen.sync(&[created(downloading("a"), 1), created(downloading("b"), 2)]);
    screen.step(1);
    screen.follow("1000-c".to_owned());
    // A poll without it leaves the selection where it was, and so does one
    // that brings some other job.
    screen.sync(&[created(downloading("a"), 1), created(downloading("b"), 2)]);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("a")
    );
    screen.sync(&[
        created(downloading("a"), 1),
        created(downloading("b"), 2),
        created(downloading("d"), 4),
    ]);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("a")
    );
    screen.sync(&[
        created(downloading("a"), 1),
        created(downloading("b"), 2),
        created(downloading("c"), 3),
        created(downloading("d"), 4),
    ]);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("c")
    );
    // Once taken, the next arrival does not move it.
    screen.sync(&[
        created(downloading("a"), 1),
        created(downloading("b"), 2),
        created(downloading("c"), 3),
        created(downloading("d"), 4),
        created(downloading("e"), 5),
    ]);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("c")
    );
}

#[test]
fn opening_lands_on_the_newest_pull_still_going() {
    let mut screen = PullsScreen::default();
    screen.sync(&[
        created(downloading("going"), 1),
        created(
            job_row(
                "done",
                PullState::Done,
                TaskState::Done("pulled done".to_owned()),
            ),
            2,
        ),
    ]);
    screen.select_newest_live();
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("going")
    );

    let mut nothing_going = PullsScreen::default();
    nothing_going.select_newest_live();
    assert_eq!(nothing_going.selected(), 0);
    assert!(nothing_going.selected_row().is_none());
}

#[test]
fn the_rate_is_read_between_two_records_and_smoothed_after() {
    let mut screen = PullsScreen::default();
    screen.sync(&[moving("x", 0, 1_000)]);
    let progress = |bytes| InstallProgress {
        bytes_downloaded: bytes,
        total_bytes: Some(1_000_000),
        ..InstallProgress::default()
    };
    assert!(screen.rate("1000-x", &progress(0)).is_none());

    // 100 kB in a second.
    assert!(screen.sync(&[moving("x", 100_000, 2_000)]));
    let rate = screen
        .rate("1000-x", &progress(100_000))
        .expect("two records");
    assert_eq!(rate.bytes_per_second, 100_000);
    assert_eq!(rate.left_ms, Some(9_000));

    // Then 300 kB in a second: the reading moves toward it, not onto it.
    screen.sync(&[moving("x", 400_000, 3_000)]);
    let rate = screen
        .rate("1000-x", &progress(400_000))
        .expect("three records");
    assert_eq!(rate.bytes_per_second, 160_000);

    // A record that has not moved on says nothing, and the reading stands
    // for a while; once it has sat unchanged past the stall window, the
    // reading is dropped rather than shown as the pace it is still at.
    assert!(!screen.sync(&[moving("x", 400_000, 3_000)]));
    screen.sync(&[polled_again(moving("x", 400_000, 3_000), 3_000 + STALL_MS)]);
    assert!(screen.rate("1000-x", &progress(400_000)).is_some());
    screen.sync(&[polled_again(moving("x", 400_000, 3_000), 3_001 + STALL_MS)]);
    assert!(screen.rate("1000-x", &progress(400_000)).is_none());
    screen.sync(&[moving("x", 500_000, 4_000 + STALL_MS)]);
    assert!(screen.rate("1000-x", &progress(500_000)).is_some());

    // A partial total gives no estimate; a stopped transfer gives no rate.
    let partial = InstallProgress {
        total_is_partial: true,
        ..progress(400_000)
    };
    assert_eq!(
        screen.rate("1000-x", &partial).map(|rate| rate.left_ms),
        Some(None)
    );
    let mut paused = moving("x", 400_000, 4_000);
    paused.pull_state = PullState::Paused;
    paused.state = TaskState::Stopped("paused".to_owned());
    assert!(screen.sync(&[paused]));
    assert!(screen.rate("1000-x", &progress(400_000)).is_none());
}

#[test]
fn a_restarted_attempt_starts_the_reading_over() {
    let mut screen = PullsScreen::default();
    screen.sync(&[moving("x", 500_000, 1_000)]);
    screen.sync(&[moving("x", 600_000, 2_000)]);
    assert!(screen.rate("1000-x", &InstallProgress::default()).is_some());
    screen.sync(&[moving("x", 10_000, 3_000)]);
    assert!(screen.rate("1000-x", &InstallProgress::default()).is_none());
    screen.sync(&[moving("x", 20_000, 4_000)]);
    assert_eq!(
        screen
            .rate("1000-x", &InstallProgress::default())
            .map(|rate| rate.bytes_per_second),
        Some(10_000)
    );
}

#[test]
fn history_is_kept_only_for_the_selected_job() {
    let mut screen = PullsScreen::default();
    screen.sync(&[created(downloading("a"), 1), created(downloading("b"), 2)]);
    assert_eq!(
        screen.selected_row().map(|row| row.reference.as_str()),
        Some("b")
    );
    assert!(!screen.history("1000-a".to_owned(), vec!["queued".to_owned()]));
    assert!(screen.history_lines().is_empty());
    assert!(screen.history("1000-b".to_owned(), vec!["queued".to_owned()]));
    assert_eq!(screen.history_lines(), ["queued"]);
    assert!(!screen.history("1000-b".to_owned(), vec!["queued".to_owned()]));
    screen.step(1);
    assert!(screen.history_lines().is_empty());
}
