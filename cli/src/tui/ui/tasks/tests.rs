use super::*;

use ratatui::style::Style;

use crate::tui::strip::{HintTargets, TaskStrip};
use crate::tui::tasks::{TaskEvent, TaskId, TaskLabel};
use crate::tui::testing::{plan, text};

fn recorded(state: TaskState) -> TaskRow {
    let mut strip = TaskStrip::default();
    strip.record(
        TaskLabel {
            verb: "remove",
            subject: "mistral".to_owned(),
        },
        state,
        0,
    );
    strip.rows()[0].clone()
}

#[test]
fn every_task_verb_is_listed() {
    use crate::support::harnesses::HARNESSES;
    use crate::tui::effect::HandOff;
    use crate::tui::testing::record;
    use std::path::PathBuf;

    let model = record("m");
    let kinds = [
        TaskKind::Scan,
        TaskKind::Warm {
            id: "m".to_owned(),
            name: "m".to_owned(),
        },
        TaskKind::WarmViaGateway {
            id: "m".to_owned(),
            name: "m".to_owned(),
            port: 1,
        },
        TaskKind::Unload {
            id: "m".to_owned(),
            name: "m".to_owned(),
        },
        TaskKind::Pull(plan("gemma3")),
        TaskKind::Remove {
            id: "m".to_owned(),
            name: "m".to_owned(),
        },
    ];
    let hand_offs = [
        HandOff::Launch {
            harness: &HARNESSES[0],
            program: PathBuf::from("/bin/true"),
            record: Box::new(model.clone()),
        },
        HandOff::Chat {
            record: Box::new(model),
        },
        HandOff::Serve,
    ];
    for kind in &kinds {
        // Exhaustive so a new variant lands here before it lands on screen.
        match kind {
            TaskKind::Scan
            | TaskKind::Warm { .. }
            | TaskKind::WarmViaGateway { .. }
            | TaskKind::Unload { .. }
            | TaskKind::Pull(_)
            | TaskKind::Remove { .. } => {}
        }
        let verb = kind.verb();
        assert!(TaskKind::VERBS.contains(&verb), "{verb} is not listed");
    }
    for hand_off in &hand_offs {
        match hand_off {
            HandOff::Launch { .. } | HandOff::Chat { .. } | HandOff::Serve => {}
        }
        let verb = hand_off.label(1).verb;
        assert!(TaskKind::VERBS.contains(&verb), "{verb} is not listed");
    }
}

#[test]
fn a_failed_row_keeps_the_reason_plain_and_offers_dismiss() {
    let row = recorded(TaskState::Failed("is warm; unload it first".to_owned()));
    let hinted = RowHints {
        dismissable: true,
        ..RowHints::default()
    };
    let line = line(&row, 120, hinted);
    assert_eq!(
        text(&line).trim_end(),
        " remove mistral  is warm; unload it first  d dismiss"
    );
    assert_eq!(line.spans[0].style, FAILED);
    let reason = line
        .spans
        .iter()
        .find(|span| span.content.contains("unload"))
        .map(|span| span.style);
    assert_eq!(reason, Some(Style::new()));
}

#[test]
fn a_download_offers_cancel_from_the_keymap() {
    let progress = InstallProgress {
        bytes_downloaded: 1 << 30,
        total_bytes: Some(4 << 30),
        ..InstallProgress::default()
    };
    let line = Line::from(download(&progress, 120, hints(&["c"])));
    assert!(
        text(&line).ends_with(&format!("  c {}  ", keymap::verb("c"))),
        "{:?}",
        text(&line)
    );
    let quiet = Line::from(download(&progress, 120, Vec::new()));
    assert!(!text(&quiet).contains(keymap::verb("c")));
}

#[test]
fn the_bar_shrinks_with_the_strip() {
    let progress = InstallProgress {
        bytes_downloaded: 2 << 30,
        total_bytes: Some(4 << 30),
        ..InstallProgress::default()
    };
    let mut strip = TaskStrip::default();
    let id = TaskId::next();
    strip.start(id, TaskKind::Pull(plan("gemma3")));
    let row = strip
        .moved(
            TaskEvent {
                id,
                state: TaskState::Downloading(progress),
            },
            0,
        )
        .cloned()
        .unwrap();
    let cancellable = RowHints {
        cancellable: true,
        ..RowHints::default()
    };
    let bar_cells = |width| {
        let line = line(&row, width, cancellable);
        assert!(line.width() <= width, "{:?} runs past {width}", text(&line));
        text(&line)
            .chars()
            .filter(|c| *c == '█' || *c == '░')
            .count()
    };
    // The row's fixed cells: the verb column, the subject, the percent,
    // the figures, the cancel hint, and the gaps between them.
    let head = 1 + label_width(&TaskKind::VERBS, 0) + 1 + row.label.subject.width() + 2;
    let figures = "2 GB of 4 GB";
    let cancel: usize = hints(&["c"]).iter().map(Span::width).sum();
    let fixed = PERCENT_WIDTH + 2 + figures.len() + 2 + cancel;
    let floor = head + fixed + MIN_BAR_WIDTH as usize;
    assert_eq!(bar_cells(120), MAX_BAR_WIDTH as usize);
    assert!(text(&line(&row, 120, cancellable)).contains(&format!("  50%  {figures}")));
    let medium = bar_cells(floor + 4);
    let bounds = MIN_BAR_WIDTH as usize..MAX_BAR_WIDTH as usize;
    assert!(bounds.contains(&medium), "{medium}");
    assert_eq!(bar_cells(floor), MIN_BAR_WIDTH as usize);
    assert_eq!(bar_cells(floor - 1), 0);
    let compact = text(&line(&row, floor - 1, cancellable));
    assert!(compact.contains(&format!("50% · {figures}  c cancel")));
    let bare = format!("50% · {figures}").width();
    let shed = text(&line(&row, head + bare + cancel - 1, cancellable));
    assert!(shed.contains(&format!("50% · {figures}")) && !shed.contains("c cancel"));
}

#[test]
fn a_done_pull_hints_only_while_it_is_selected() {
    let mut strip = TaskStrip::default();
    let id = TaskId::next();
    strip.start(id, TaskKind::Pull(plan("gemma3")));
    strip.moved(
        TaskEvent {
            id,
            state: TaskState::Done("pulled gemma3".to_owned()),
        },
        0,
    );
    let shown = strip.shown(10);
    let selected = strip.hint_targets(10, |reference| reference == "gemma3");
    let line_for = |targets: HintTargets| {
        text(&line(shown[0], 120, targets.for_row(shown[0])))
            .trim_end()
            .to_owned()
    };
    assert!(line_for(selected).ends_with("pulled gemma3  w warm  l launch"));
    let elsewhere = strip.hint_targets(10, |reference| reference == "llava");
    assert!(line_for(elsewhere).ends_with("pulled gemma3"));
}

/// A strip with `failed` failed scans, then `done` finished ones, then
/// `running` pulls still downloading, oldest first.
fn strip_of(failed: usize, done: usize, running: usize) -> TaskStrip {
    let mut strip = TaskStrip::default();
    for index in 0..failed {
        let id = TaskId::next();
        strip.start(id, TaskKind::Scan);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Failed(format!("failed {index}")),
            },
            0,
        );
    }
    for index in 0..done {
        let id = TaskId::next();
        strip.start(id, TaskKind::Scan);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Done(format!("done {index}")),
            },
            0,
        );
    }
    for index in 0..running {
        let id = TaskId::next();
        strip.start(id, TaskKind::Pull(plan(&format!("pull-{index}"))));
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Downloading(InstallProgress::default()),
            },
            0,
        );
    }
    strip
}

fn rendered(strip: &TaskStrip, height: usize) -> Vec<String> {
    let shown = strip.shown(height);
    let targets = strip.hint_targets(height, |_| false);
    shown
        .iter()
        .map(|row| {
            text(&line(row, 120, targets.for_row(row)))
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn cancel_sits_on_the_newest_pull_even_while_it_resolves() {
    let mut strip = strip_of(0, 0, 1);
    let id = TaskId::next();
    strip.start(id, TaskKind::Pull(plan("pull-b")));
    let lines = rendered(&strip, 10);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("pull-0") && !lines[0].contains("cancel"));
    assert!(
        lines[1].ends_with("pull-b  starting  c cancel"),
        "{:?}",
        lines[1]
    );
    strip.moved(
        TaskEvent {
            id,
            state: TaskState::Status("resolving on hf".to_owned()),
        },
        0,
    );
    let lines = rendered(&strip, 10);
    assert!(
        lines[1].ends_with("resolving on hf  c cancel"),
        "{:?}",
        lines[1]
    );
    let mut strip = TaskStrip::default();
    strip.start(TaskId::next(), TaskKind::Scan);
    assert!(!rendered(&strip, 10)[0].contains("cancel"));
}

#[test]
fn a_failure_off_screen_offers_no_dismiss() {
    let strip = strip_of(1, 4, 0);
    let lines = rendered(&strip, 4);
    assert!(
        lines.iter().all(|line| !line.contains("dismiss")),
        "{lines:?}"
    );
    assert!(rendered(&strip, 5)[0].ends_with("failed 0  d dismiss"));
}

#[test]
fn only_the_newest_failure_offers_dismiss() {
    let strip = strip_of(2, 0, 2);
    let lines = rendered(&strip, 10);
    assert_eq!(lines.len(), 4);
    assert!(lines[0].ends_with("failed 0") && !lines[0].contains("dismiss"));
    assert!(lines[1].ends_with("failed 1  d dismiss"));
    assert!(lines[2].contains("so far") && !lines[2].contains("cancel"));
    assert!(lines[3].ends_with("so far  c cancel"), "{:?}", lines[3]);
}

#[test]
fn a_running_download_survives_the_cap() {
    let mut strip = strip_of(0, 0, 1);
    for index in 0..5 {
        let id = TaskId::next();
        strip.start(id, TaskKind::Scan);
        strip.moved(
            TaskEvent {
                id,
                state: TaskState::Done(format!("done {index}")),
            },
            0,
        );
    }
    let lines = rendered(&strip, 4);
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with(" pull") && lines[0].ends_with("c cancel"));
    assert!(lines[1].ends_with("done 2"));
    assert!(lines[3].ends_with("done 4"));
    let all = rendered(&strip, 10);
    assert_eq!(all.len(), 6);
    assert!(all[0].starts_with(" pull"));
    let crowded = strip_of(0, 1, 3);
    let lines = rendered(&crowded, 2);
    assert!(lines.iter().all(|line| line.starts_with(" pull")));
    assert!(lines[1].contains("pull-2"));
}
