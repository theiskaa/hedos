use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::tui::app::{App, Modal};
use crate::tui::event::{Event, Key};
use crate::tui::facts::Facts;
use crate::tui::pull::PullModal;
use crate::tui::testing::{facts_with_memory, plan, record};

/// The whole frame of `app` at `width` by `height`, row by row.
fn frame(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, app))
        .expect("a frame");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

fn opened(facts: Facts) -> App {
    let mut app = App::new(vec![record("m")], facts);
    app.reduce(Event::Key(Key::Char('p')));
    app
}

#[test]
fn the_screen_lays_out_search_kinds_results_and_preview() {
    let mut app = opened(facts_with_memory(64));
    let rows = frame(&mut app, 132, 42);
    let all = rows.join("\n");
    assert!(
        all.contains("search by name, owner/repo or name:tag"),
        "{all}"
    );
    assert!(all.contains("✓ catalog"));
    assert!(all.contains(" all "));
    assert!(all.contains("speech"));
    assert!(all.contains("─ results ─"));
    assert!(all.contains("recommended for 64 GiB"));
    assert!(all.contains("─ preview ─"));
    assert!(all.contains("─ downloads · 0 ─"));
    assert!(all.contains("nothing downloading"));
    assert!(
        rows[41].contains("enter pull") && rows[41].contains("esc back"),
        "{:?}",
        rows[41]
    );
    assert!(!rows[41].contains("q quit"), "q types into the search here");
}

#[test]
fn a_ready_row_offers_the_button_with_its_size() {
    let mut app = App::new(vec![record("m")], facts_with_memory(64));
    let mut ready = plan("gemma3");
    ready.total_bytes = Some(5_000_000_000);
    app.modal = Some(Modal::Pull(Box::new(PullModal::ready(ready))));
    let all = frame(&mut app, 132, 42).join("\n");
    assert!(all.contains("enter   pull 5 GB"), "{all}");
    assert!(all.contains("nothing moves until you press enter"));
}

#[test]
fn the_screen_never_panics_and_keeps_inside_small_terminals() {
    for (width, height) in [(80, 24), (100, 30), (60, 16), (20, 5), (1, 1)] {
        let mut app = opened(facts_with_memory(16));
        let rows = frame(&mut app, width, height);
        assert_eq!(rows.len(), height as usize);
    }
    let mut app = opened(facts_with_memory(16));
    let all = frame(&mut app, 80, 24).join("\n");
    assert!(
        all.contains("─ preview ─"),
        "the preview sits under the results: {all}"
    );
}
