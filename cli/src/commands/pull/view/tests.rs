use super::*;

use kernel::install::pulls::PullState;

use crate::support::pulls::testing::{held, moved, status};

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
