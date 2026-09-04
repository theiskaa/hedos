use super::*;

use kernel::install::event::InstallProgress;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::tui::facts::Facts;
use crate::tui::tasks::TaskState;
use crate::tui::testing::{downloading, job_row, texts};

/// The screen's list and detail drawn into `width` by `height`, as text rows.
fn rendered(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| {
            let [list, detail] = ratatui::layout::Layout::horizontal([
                Constraint::Percentage(55),
                Constraint::Min(0),
            ])
            .areas(frame.area());
            draw_list(frame, list, app);
            draw_detail(frame, detail, app);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect()
}

fn app_with(rows: Vec<JobRow>) -> App {
    let mut app = App::new(Vec::new(), Facts::default());
    app.pulls.sync(&rows);
    app.pulls.select_newest_live();
    app
}

#[test]
fn the_list_has_the_ls_columns_and_marks_the_selection() {
    let mut moving = downloading("Qwen/Qwen3-8B");
    moving.status.progress = InstallProgress {
        bytes_downloaded: 1 << 30,
        total_bytes: Some(4 << 30),
        ..InstallProgress::default()
    };
    moving.descriptor.created_at_ms = 2;
    let mut app = app_with(vec![
        job_row(
            "a/paused",
            PullState::Paused,
            TaskState::Stopped("paused".to_owned()),
        ),
        moving,
    ]);
    let lines = rendered(&mut app, 120, 10);
    assert!(lines[0].contains(" pulls "));
    assert!(
        lines[1].contains("REFERENCE")
            && lines[1].contains("STATE")
            && lines[1].contains("PROGRESS")
    );
    // Newest first.
    assert!(lines[2].contains("Qwen/Qwen3-8B") && lines[2].contains("running"));
    assert!(lines[2].contains("25%  1 GB of 4 GB"));
    assert!(lines[2].contains(SELECTED_MARK));
    assert!(lines[3].contains("a/paused") && lines[3].contains("paused"));
    assert!(!lines[3].contains(SELECTED_MARK));
}

#[test]
fn a_narrow_list_cuts_the_reference_before_it_drops_the_progress() {
    let widths = [1, 30, 11, 22];
    // 1 + 12 + 11 + 22 + 6 = 52 inside the borders.
    assert_eq!(fitting_columns(&widths, 54), &[0, 1, 2, 3]);
    assert_eq!(fitting_columns(&widths, 53), &[0, 1, 2]);
}

#[test]
fn the_detail_reads_the_record_and_the_history_newest_last() {
    let mut moving = downloading("Qwen/Qwen3-8B");
    moving.status.attempt = 2;
    moving.note = "attempt 2".to_owned();
    moving.started_ago = "3m".to_owned();
    moving.updated_ago = "1s".to_owned();
    let mut app = app_with(vec![moving]);
    let history: Vec<String> = (1..=6).map(|n| format!("{n}s ago  event {n}")).collect();
    app.pulls.history("1000-Qwen/Qwen3-8B".to_owned(), history);
    let row = app.pulls.selected_row().expect("selected").clone();

    let all = lines(&row, &app.pulls, 60, 30);
    let shown = texts(&all);
    assert!(shown[0].starts_with(" state") && shown[0].ends_with("running · attempt 2"));
    // Where it is comes first, so a stacked pane's four rows say what matters.
    assert!(shown[1].starts_with(" progress"));
    assert!(shown[2].starts_with(" note") && shown[2].ends_with("attempt 2"));
    assert!(shown[3].starts_with(" id") && shown[3].ends_with("1000-Qwen/Qwen3-8B"));
    assert!(shown[4].ends_with("ollama · Qwen/Qwen3-8B"));
    assert!(shown[5].starts_with(" to") && shown[5].ends_with("/models/Qwen/Qwen3-8B"));
    assert!(shown.iter().any(|line| line.ends_with("3m ago")));
    assert!(!shown.iter().any(|line| line.starts_with(" rate")));
    assert!(shown.contains(&" HISTORY".to_owned()));
    assert!(shown.last().is_some_and(|line| line.ends_with("event 6")));
    assert!(all.iter().all(|line| line.width() <= 60));

    // Eleven rows: the record takes eight, the heading two, one history line
    // fits.
    let few = texts(&lines(&row, &app.pulls, 60, 11));
    assert_eq!(few.len(), 11);
    assert!(few.last().is_some_and(|line| line.ends_with("event 6")));
    assert!(!few.iter().any(|line| line.ends_with("event 5")));
    // Ten: no room for a line under the heading, so the heading goes too.
    let none = texts(&lines(&row, &app.pulls, 60, 10));
    assert_eq!(none.len(), 8);
    assert!(!none.contains(&" HISTORY".to_owned()));
}

#[test]
fn the_rate_line_shows_once_two_records_were_read() {
    let record = |bytes, at_ms| {
        let mut row = downloading("x");
        row.status.progress = InstallProgress {
            bytes_downloaded: bytes,
            total_bytes: Some(4 << 20),
            ..InstallProgress::default()
        };
        row.status.updated_at_ms = at_ms;
        row
    };
    let mut app = app_with(vec![record(0, 1_000)]);
    let mut later = record(1 << 20, 2_000);
    later.polled_at_ms = 2_000;
    app.pulls.sync(&[later]);
    let row = app.pulls.selected_row().expect("selected").clone();
    let shown = texts(&lines(&row, &app.pulls, 60, 20));
    let rate = shown
        .iter()
        .find(|line| line.starts_with(" rate"))
        .expect("a rate line");
    assert!(rate.ends_with("1 MB/s · 3s left"), "{rate}");
}

#[test]
fn an_empty_store_says_where_a_pull_starts() {
    let mut app = app_with(Vec::new());
    let lines = rendered(&mut app, 100, 8);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("no pulls yet · p on the shelf pulls one"))
    );
    assert!(lines[0].contains(" pull "));
}
