use super::*;

use kernel::install::pulls::{PullState, START_GRACE_MS};

use crate::support::pulls::testing::{TempDir, held, job, moved, shown, status, unreadable_job};

#[test]
fn the_table_lines_its_columns_up_under_a_header() {
    let held = held("view-table");
    let table = table(
        &[(
            held.job.clone(),
            shown(moved(1 << 30, Some(4 << 30), false)),
        )],
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
    let value = json(&held.job, &shown(moved(64, Some(128), false)));

    assert_eq!(value["reference"], "Qwen/Qwen3-8B");
    assert_eq!(value["provider"], "huggingface");
    assert_eq!(value["state"], "running");
    assert_eq!(value["progress"]["bytes_downloaded"], 64);
    assert_eq!(value["id"], held.job.id());
    assert!(value.get("abandoned").is_none());
}

#[test]
fn an_abandoned_pull_lists_as_interrupted_with_no_worker() {
    let directory = TempDir::new("view-abandoned");
    let job = job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
    let now = 1_000 + START_GRACE_MS;

    let table = table(&[(job.clone(), job.reading(now))], now);

    let row = table.lines().nth(1).expect("one job row");
    assert!(row.contains("interrupted"), "{row}");
    assert!(row.contains("no worker"), "{row}");
    assert!(!row.contains("queued"), "{row}");
}

#[test]
fn json_marks_a_pull_no_worker_took_up() {
    let directory = TempDir::new("view-abandoned-json");
    let job = job(&directory.store(), "Qwen/Qwen3-8B", 1_000);

    let value = json(&job, &job.reading(1_000 + START_GRACE_MS));

    assert_eq!(value["state"], "interrupted");
    assert_eq!(value["abandoned"], true);

    let fresh = json(&job, &job.reading(1_000));
    assert_eq!(fresh["state"], "queued");
    assert!(fresh.get("abandoned").is_none());
}

#[test]
fn the_commands_that_reach_a_pull_stand_on_their_own_after_a_line_that_named_it() {
    let held = held("view-reach");
    let reach = reach(&held.job);
    assert!(reach.starts_with("  watch:  hedos pull attach "));
    assert!(reach.contains(&format!("stop:   hedos pull cancel {}", held.job.id())));
    assert!(!reach.contains("pulling"), "the pull was named already");
    assert!(detached(&held.job).ends_with(&reach));
}

#[test]
fn an_unreadable_record_lists_as_unreadable() {
    let directory = TempDir::new("view-unreadable");
    let job = unreadable_job(&directory.store(), "Qwen/Qwen3-8B", 1_000);
    let now = 1_000 + START_GRACE_MS;

    let table = table(&[(job.clone(), job.reading(now))], now);
    let value = json(&job, &job.reading(now));

    let row = table.lines().nth(1).expect("one job row");
    assert!(row.contains("unreadable"), "{row}");
    assert!(row.contains("its record could not be"), "{row}");
    assert_eq!(value["state"], "unreadable");
    assert!(value.get("abandoned").is_none());
}
