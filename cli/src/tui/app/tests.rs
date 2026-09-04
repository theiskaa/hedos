use super::*;

use crate::tui::keymap;
use crate::tui::strip::{DONE_LINGER_TICKS, FAILED_LINGER_TICKS};
use crate::tui::testing::{downloading, job_row, plan, resident};
use kernel::install::pulls::PullState;
use kernel::records::{Capability, Modality, ModelSource, SourceKind};

fn record(index: usize) -> ModelRecord {
    crate::tui::testing::record(&format!("model-{index}"))
}

fn app(count: usize) -> App {
    app_from((0..count).map(record).collect())
}

fn app_from(records: Vec<ModelRecord>) -> App {
    App::new(records, Facts::default())
}

fn press(app: &mut App, key: Key) -> Vec<Effect> {
    app.reduce(Event::Key(key))
}

fn pull(app: &App) -> &PullModal {
    match &app.modal {
        Some(Modal::Pull(modal)) => modal,
        _ => panic!("no pull modal"),
    }
}

fn ticks(app: &mut App, count: u64) -> Vec<Effect> {
    (0..count).flat_map(|_| app.reduce(Event::Tick)).collect()
}

#[test]
fn movement_clamps_at_both_ends() {
    let mut app = app(3);
    press(&mut app, Key::Up);
    assert_eq!(app.selected(), 0);
    for _ in 0..5 {
        press(&mut app, Key::Char('j'));
    }
    assert_eq!(app.selected(), 2);
}

#[test]
fn top_and_bottom_jump() {
    let mut app = app(4);
    press(&mut app, Key::Char('G'));
    assert_eq!(app.selected(), 3);
    press(&mut app, Key::Char('g'));
    assert_eq!(app.selected(), 0);
}

#[test]
fn selected_is_matches_by_the_pull_modal_rule() {
    let mut app = app(2);
    press(&mut app, Key::Down);
    assert_eq!(
        app.selected_record().map(|r| r.name.as_str()),
        Some("model-1")
    );
    assert!(app.selected_is("model-1"));
    assert!(app.selected_is("owner/model-1"));
    assert!(app.selected_is("MODEL-1"));
    assert!(!app.selected_is("model-0"));
    assert!(!app.selected_is("model-2"));
}

#[test]
fn an_empty_shelf_never_selects() {
    let mut app = app(0);
    press(&mut app, Key::Down);
    assert_eq!(app.selected(), 0);
    assert!(app.selected_record().is_none());
    assert!(press(&mut app, Key::Char('w')).is_empty());
}

#[test]
fn quit_keys_yield_quit() {
    let mut app = app(1);
    assert_eq!(press(&mut app, Key::Char('q')), vec![Effect::Quit]);
    assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
}

#[test]
fn only_changes_mark_the_screen_dirty() {
    let mut app = app(2);
    assert!(app.take_dirty());
    press(&mut app, Key::Up);
    assert!(!app.take_dirty());
    press(&mut app, Key::Down);
    assert!(app.take_dirty());
    app.reduce(Event::Resize);
    assert!(app.take_dirty());
    // A key a modal does not answer leaves the screen as it was.
    for (open, unhandled) in [
        ('p', Key::PageUp),
        ('x', Key::Char('z')),
        ('l', Key::PageUp),
        ('?', Key::Char('j')),
        ('t', Key::Enter),
    ] {
        press(&mut app, Key::Char(open));
        assert!(app.take_dirty(), "{open} opens a modal");
        assert!(press(&mut app, unhandled).is_empty());
        assert!(!app.take_dirty(), "{unhandled:?} redraws the {open} modal");
        app.modal = None;
    }
}

#[test]
fn scan_and_refresh_are_effects() {
    let mut app = app(1);
    assert_eq!(
        press(&mut app, Key::Char('s')),
        vec![Effect::Spawn(TaskKind::Scan)]
    );
    assert_eq!(press(&mut app, Key::Char('r')), vec![Effect::Refresh]);
}

#[test]
fn warm_goes_local_without_a_gateway_and_through_it_with_one() {
    let mut app = app(1);
    let id = app.records[0].id.clone();
    assert_eq!(
        press(&mut app, Key::Char('w')),
        vec![Effect::Spawn(TaskKind::Warm {
            id,
            name: "model-0".to_owned()
        })]
    );
    app.facts.gateway_port = Some(4321);
    assert!(matches!(
        press(&mut app, Key::Char('w')).as_slice(),
        [Effect::Spawn(TaskKind::WarmViaGateway { port: 4321, .. })]
    ));
    app.started(
        TaskId::next(),
        TaskKind::WarmViaGateway {
            id: app.records[0].id.clone(),
            name: "model-0".to_owned(),
            port: 4321,
        },
    );
    assert!(press(&mut app, Key::Char('w')).is_empty());
}

#[test]
fn warming_a_warm_model_only_notifies() {
    let mut app = app(1);
    app.facts
        .residents
        .push(resident(&app.records[0].id, Holder::Local));
    assert!(press(&mut app, Key::Char('w')).is_empty());
    assert_eq!(app.notice(), Some("model-0 is already warm"));
    ticks(&mut app, NOTICE_TICKS);
    assert_eq!(app.notice(), None);
}

#[test]
fn unload_needs_a_locally_warm_model() {
    let mut app = app(1);
    let id = app.records[0].id.clone();
    assert!(press(&mut app, Key::Char('u')).is_empty());
    assert_eq!(app.notice(), Some("model-0 is not warm"));

    app.facts.residents.push(resident(&id, Holder::Gateway));
    assert!(press(&mut app, Key::Char('u')).is_empty());
    assert!(app.notice().unwrap().contains("held by the gateway"));

    app.facts.residents[0].holder = Holder::Local;
    assert_eq!(
        press(&mut app, Key::Char('u')),
        vec![Effect::Spawn(TaskKind::Unload {
            id,
            name: "model-0".to_owned()
        })]
    );
}

#[test]
fn a_running_task_blocks_a_duplicate_on_the_same_model() {
    let mut app = app(1);
    let id = app.records[0].id.clone();
    app.started(
        TaskId::next(),
        TaskKind::Warm {
            id,
            name: "model-0".to_owned(),
        },
    );
    assert!(press(&mut app, Key::Char('w')).is_empty());
    assert!(app.busy());
}

#[test]
fn finished_tasks_request_a_refresh_and_results_fade() {
    let mut app = app(1);
    let id = TaskId::next();
    app.started(id, TaskKind::Scan);
    app.take_dirty();
    let effects = app.reduce(Event::Task(TaskEvent {
        id,
        state: TaskState::Done("found 2".to_owned()),
    }));
    assert_eq!(effects, vec![Effect::Refresh]);
    assert!(app.take_dirty());
    assert_eq!(app.tasks.rows().len(), 1);
    ticks(&mut app, DONE_LINGER_TICKS + 1);
    assert!(app.tasks.rows().is_empty());
}

#[test]
fn failures_stay_much_longer_than_results() {
    let mut app = app(1);
    let id = TaskId::next();
    app.started(id, TaskKind::Scan);
    app.reduce(Event::Task(TaskEvent {
        id,
        state: TaskState::Failed("no".to_owned()),
    }));
    ticks(&mut app, DONE_LINGER_TICKS * 2);
    assert_eq!(app.tasks.rows().len(), 1);
    ticks(&mut app, FAILED_LINGER_TICKS);
    assert!(app.tasks.rows().is_empty());
}

#[test]
fn idle_refresh_is_slower_than_busy_refresh() {
    // The job directory is polled on its own cadence, which this is not about.
    let refreshes = |effects: Vec<Effect>| {
        effects
            .into_iter()
            .filter(|effect| *effect == Effect::Refresh)
            .collect::<Vec<_>>()
    };
    let mut app = app(1);
    let idle = refreshes(ticks(&mut app, IDLE_REFRESH_TICKS));
    assert_eq!(idle, vec![Effect::Refresh]);
    app.started(TaskId::next(), TaskKind::Scan);
    let busy = refreshes(ticks(&mut app, BUSY_REFRESH_TICKS));
    assert_eq!(busy, vec![Effect::Refresh]);
}

#[test]
fn the_job_directory_is_read_more_often_while_a_pull_is_going() {
    let polls = |effects: Vec<Effect>| {
        effects
            .iter()
            .filter(|effect| **effect == Effect::PollPulls)
            .count()
    };
    let mut app = app(1);
    let idle = polls(ticks(&mut app, PULL_IDLE_POLL_TICKS * 4));
    app.reduce(Event::Pulls(vec![downloading("x")]));
    let going = polls(ticks(&mut app, PULL_IDLE_POLL_TICKS * 4));

    assert!(
        going > idle,
        "a download that is moving should be read more often: {going} against {idle}"
    );
}

#[test]
fn a_refresh_keeps_the_selected_model() {
    let mut app = app(3);
    press(&mut app, Key::Char('G'));
    let kept = app.records[2].clone();
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 2,
        records: vec![kept.clone(), record(9)],
        facts: Facts::default(),
    }));
    assert_eq!(app.selected_record().map(|r| &r.id), Some(&kept.id));

    app.reduce(Event::Refreshed(Refreshed {
        sequence: 3,
        records: Vec::new(),
        facts: Facts::default(),
    }));
    assert!(app.selected_record().is_none());
}

#[test]
fn the_pull_modal_captures_keys_until_it_closes() {
    let mut app = app(1);
    press(&mut app, Key::Char('p'));
    assert!(app.modal.is_some());
    assert!(press(&mut app, Key::Char('q')).is_empty());
    assert_eq!(pull(&app).input.as_str(), "q");
    press(&mut app, Key::Backspace);
    let effects = press(&mut app, Key::Enter);
    assert!(matches!(effects.as_slice(), [Effect::Plan(_, _, _)]));
    press(&mut app, Key::Escape);
    assert_eq!(pull(&app).stage, Stage::Listing);
    press(&mut app, Key::Escape);
    assert!(app.modal.is_none());
    assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
}

#[test]
fn ticks_turn_the_planning_spinner_and_nothing_else() {
    let mut app = app(1);
    press(&mut app, Key::Char('p'));
    app.take_dirty();
    assert!(ticks(&mut app, 1).is_empty());
    assert!(!app.take_dirty());
    press(&mut app, Key::Enter);
    assert!(matches!(pull(&app).stage, Stage::Planning(_)));
    app.take_dirty();
    assert!(ticks(&mut app, 1).is_empty());
    assert!(app.take_dirty());
    press(&mut app, Key::Escape);
    app.take_dirty();
    ticks(&mut app, 1);
    assert!(!app.take_dirty());
}

#[test]
fn a_typed_query_is_searched_after_the_debounce() {
    let mut app = app(1);
    press(&mut app, Key::Char('p'));
    press(&mut app, Key::Char('x'));
    assert!(ticks(&mut app, 1).is_empty());
    assert_eq!(ticks(&mut app, 1), vec![Effect::Search("x".to_owned())]);
}

#[test]
fn an_abandoned_partial_download_can_be_removed() {
    // `downloading` is discovery's reading of incomplete blobs on disk, not
    // a live pull; with no task running there is nothing to cancel, so
    // removal is the only way out.
    let mut partial = record(0);
    partial.downloading = true;
    let mut app = app_from(vec![partial]);
    assert!(press(&mut app, Key::Char('x')).is_empty());
    assert!(app.notice().is_none());
    assert!(matches!(app.modal, Some(Modal::Remove(_))));
}

/// One pull of `reference` in `state`, as a poll of the job directory reports
/// it.
fn polled(reference: &str, place: PullState, state: TaskState) -> Event {
    Event::Pulls(vec![job_row(reference, place, state)])
}

#[test]
fn cancel_targets_the_newest_pull_still_going() {
    let mut app = app(1);
    assert!(press(&mut app, Key::Char('c')).is_empty());
    assert_eq!(app.notice(), Some("nothing is downloading"));
    app.reduce(polled(
        "x",
        PullState::Queued,
        TaskState::Status("queued".to_owned()),
    ));
    assert_eq!(
        press(&mut app, Key::Char('c')),
        vec![Effect::ControlPull(PullAction::Cancel, "1000-x".to_owned())]
    );
    // Newer running rows push the pull off the strip, where `c` cannot reach it.
    for _ in 0..layout::MAX_TASK_ROWS {
        app.started(TaskId::next(), TaskKind::Scan);
    }
    assert!(press(&mut app, Key::Char('c')).is_empty());
    assert_eq!(app.notice(), Some("nothing is downloading"));
    app.tasks = TaskStrip::default();
    app.reduce(polled(
        "x",
        PullState::Queued,
        TaskState::Status("queued".to_owned()),
    ));

    let mut modal = PullModal::open(&[], 0, &[]);
    let mut replanned = plan("x");
    replanned.remaining_bytes = Some(5);
    modal.stage = Stage::Preview(replanned);
    app.modal = Some(Modal::Pull(Box::new(modal)));
    assert!(press(&mut app, Key::Enter).is_empty());
    assert_eq!(app.notice(), Some("x is already downloading"));
}

#[test]
fn resume_targets_the_newest_pull_that_stopped() {
    let mut app = app(1);
    assert!(press(&mut app, Key::Char('R')).is_empty());
    assert_eq!(app.notice(), Some("no pull is waiting to go on"));

    // A pull still going answers `c`, not `R`; only a stopped one offers to be
    // carried on.
    app.reduce(Event::Pulls(vec![downloading("x")]));
    assert!(press(&mut app, Key::Char('R')).is_empty());

    app.reduce(polled(
        "x",
        PullState::Paused,
        TaskState::Stopped("paused".to_owned()),
    ));
    assert_eq!(
        press(&mut app, Key::Char('R')),
        vec![Effect::ControlPull(PullAction::Resume, "1000-x".to_owned())]
    );
}

#[test]
fn starting_a_pull_hands_the_plan_to_a_worker() {
    let mut app = app(1);
    let mut modal = PullModal::open(&[], 0, &[]);
    let plan = plan("gemma3");
    modal.stage = Stage::Preview(plan.clone());
    app.modal = Some(Modal::Pull(Box::new(modal)));

    let effects = press(&mut app, Key::Enter);

    assert_eq!(effects, vec![Effect::StartPull(Box::new(plan))]);
    assert!(app.modal.is_none());
}

#[test]
fn remove_asks_first_and_refuses_warm_models() {
    let mut app = app(1);
    let id = app.records[0].id.clone();
    app.facts.residents.push(resident(&id, Holder::Local));
    assert!(press(&mut app, Key::Char('x')).is_empty());
    assert_eq!(app.notice(), Some("model-0 is warm; unload it first"));
    app.facts.residents.clear();

    press(&mut app, Key::Char('x'));
    assert!(matches!(app.modal, Some(Modal::Remove(_))));
    assert!(press(&mut app, Key::Char('n')).is_empty());
    assert!(app.modal.is_none());

    press(&mut app, Key::Char('x'));
    assert_eq!(
        press(&mut app, Key::Char('y')),
        vec![Effect::Spawn(TaskKind::Remove {
            id: id.clone(),
            name: "model-0".to_owned()
        })]
    );
    assert!(app.modal.is_none());

    press(&mut app, Key::Char('x'));
    app.facts.residents.push(resident(&id, Holder::Gateway));
    assert!(press(&mut app, Key::Char('y')).is_empty());
    assert_eq!(app.notice(), Some("model-0 is warm; unload it first"));
    assert!(app.modal.is_none());
}

#[test]
fn enter_expands_the_detail_and_escape_folds_it() {
    let mut one = app(1);
    press(&mut one, Key::Enter);
    assert!(one.expanded);
    press(&mut one, Key::Escape);
    assert!(!one.expanded);
    let mut empty = app(0);
    press(&mut empty, Key::Enter);
    assert!(!empty.expanded);
    press(&mut one, Key::Enter);
    one.reduce(Event::Refreshed(Refreshed {
        sequence: 9,
        records: Vec::new(),
        facts: Facts::default(),
    }));
    assert!(!one.expanded);
}

#[test]
fn the_filter_narrows_the_shelf_and_escape_clears_it() {
    let mut app = app(3);
    press(&mut app, Key::Char('/'));
    assert!(app.filtering);
    press(&mut app, Key::Char('2'));
    assert_eq!(app.order, vec![2]);
    assert_eq!(
        app.selected_record().map(|r| r.name.as_str()),
        Some("model-2")
    );
    press(&mut app, Key::Enter);
    assert!(!app.filtering);
    assert_eq!(app.order.len(), 1);
    press(&mut app, Key::Escape);
    assert_eq!(app.order.len(), 3);
    assert_eq!(
        app.selected_record().map(|r| r.name.as_str()),
        Some("model-2")
    );
}

#[test]
fn sort_cycles_and_keeps_the_selection() {
    let mut app = app(3);
    app.records[0].footprint_mb = Some(1);
    app.records[2].footprint_mb = Some(9);
    press(&mut app, Key::Char('o'));
    assert_eq!(app.sort, Sort::Size);
    assert_eq!(app.order[0], 2);
    assert_eq!(
        app.selected_record().map(|r| r.name.as_str()),
        Some("model-0")
    );
}

#[test]
fn copy_yields_the_path_or_a_notice() {
    let mut app = app(1);
    assert!(press(&mut app, Key::Char('y')).is_empty());
    assert_eq!(app.notice(), Some("model-0 has no path"));
    app.records[0].primary_weight_path = Some("/w".to_owned());
    assert_eq!(
        press(&mut app, Key::Char('y')),
        vec![Effect::Copy("/w".to_owned())]
    );
    let id = app.records[0].id.clone();
    assert_eq!(press(&mut app, Key::Char('Y')), vec![Effect::Copy(id)]);
}

#[test]
fn dismiss_drops_the_newest_failure() {
    let mut app = app(1);
    let id = TaskId::next();
    app.started(id, TaskKind::Scan);
    app.reduce(Event::Task(TaskEvent {
        id,
        state: TaskState::Failed("no".to_owned()),
    }));
    press(&mut app, Key::Char('d'));
    assert!(app.tasks.rows().is_empty());
    press(&mut app, Key::Char('d'));
    assert_eq!(app.notice(), Some("nothing to dismiss"));
}

#[test]
fn dismiss_leaves_a_failure_the_strip_does_not_show() {
    let mut app = app(1);
    let failed = TaskId::next();
    app.started(failed, TaskKind::Scan);
    app.reduce(Event::Task(TaskEvent {
        id: failed,
        state: TaskState::Failed("no".to_owned()),
    }));
    for _ in 0..layout::MAX_TASK_ROWS {
        let id = TaskId::next();
        app.started(id, TaskKind::Scan);
        app.reduce(Event::Task(TaskEvent {
            id,
            state: TaskState::Done("ok".to_owned()),
        }));
    }
    app.notice = None;
    press(&mut app, Key::Char('d'));
    assert_eq!(app.notice(), Some("nothing to dismiss"));
    assert_eq!(app.tasks.rows().len(), 1 + layout::MAX_TASK_ROWS as usize);
    assert_eq!(app.tasks.newest_failure(), Some(failed));
}

#[test]
fn state_round_trips_through_restore() {
    let mut app = app(3);
    press(&mut app, Key::Char('G'));
    let state = app.remembered();
    let mut fresh = app_from(app.records.clone());
    fresh.restore(&state);
    assert_eq!(
        fresh.selected_record().map(|r| &r.id),
        Some(&app.records[2].id)
    );
}

#[test]
fn help_closes_on_escape_question_mark_or_q_only() {
    for closer in [Key::Escape, Key::Char('?'), Key::Char('q')] {
        let mut app = app(3);
        press(&mut app, Key::Down);
        assert_eq!(app.selected(), 1);
        press(&mut app, Key::Char('?'));
        assert_eq!(app.modal, Some(Modal::Help));
        assert!(press(&mut app, Key::Char('j')).is_empty());
        assert_eq!(app.modal, Some(Modal::Help));
        assert_eq!(app.selected(), 1);
        assert!(press(&mut app, closer).is_empty());
        assert!(app.modal.is_none());
    }
}

#[test]
fn launch_offers_harnesses_and_hands_off_on_an_allowed_one() {
    let mut app = app(1);
    press(&mut app, Key::Char('l'));
    assert!(matches!(app.modal, Some(Modal::Launch(_))));
    press(&mut app, Key::Escape);
    assert!(app.modal.is_none());
    let record = record(0);
    app.open(Modal::Launch(Box::new(LaunchModal::open_with(
        &record,
        |_| Some(std::path::PathBuf::from("/bin/harness")),
    ))));
    let effects = press(&mut app, Key::Enter);
    assert!(matches!(
        effects.as_slice(),
        [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Launch { .. })
    ));
    assert!(app.modal.is_none());
    app.open(Modal::Launch(Box::new(LaunchModal::open_with(
        &record,
        |_| None,
    ))));
    assert!(press(&mut app, Key::Enter).is_empty());
    assert!(
        app.notice()
            .is_some_and(|notice| notice.contains("not installed"))
    );
}

#[test]
fn the_scroll_keys_reach_the_pane() {
    let mut app = app(1);
    let generation = ask(&mut app);
    reply(&mut app, generation, ReplyStep::Text("a\n".repeat(40)));
    app.chat_pane_mut().expect("the pane").measured(30);
    press(&mut app, Key::PageUp);
    assert_eq!(
        app.chat_pane().expect("the pane").first_line(),
        30 - PAGE_LINES
    );
    press(&mut app, Key::ScrollDown);
    assert_eq!(
        app.chat_pane().expect("the pane").first_line(),
        30 - PAGE_LINES + WHEEL_LINES
    );
    press(&mut app, Key::Bottom);
    assert_eq!(app.chat_pane().expect("the pane").first_line(), 30);
}

#[test]
fn launch_is_refused_for_a_model_that_cannot_chat() {
    let mut app = app(1);
    app.records[0].capabilities = vec![Capability::speak()];
    app.reorder_in_place();
    assert!(press(&mut app, Key::Char('l')).is_empty());
    assert_eq!(
        app.notice(),
        Some("model-0 can't chat, so no harness can use it")
    );
}

#[test]
fn coming_back_adds_a_finished_row_and_keeps_the_selection() {
    let mut app = app(3);
    press(&mut app, Key::Char('G'));
    let kept = app.records[2].clone();
    app.came_back(
        Refreshed {
            sequence: 5,
            records: vec![kept.clone(), record(7)],
            facts: Facts::default(),
        },
        TaskLabel {
            verb: "launch",
            subject: "x".to_owned(),
        },
        TaskState::Done("ran 4m".to_owned()),
    );
    assert_eq!(app.selected_record().map(|r| &r.id), Some(&kept.id));
    assert_eq!(app.tasks.rows().len(), 1);
    assert!(!app.busy());
    // A refresh that was in flight before leaving is older and must lose.
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 4,
        records: Vec::new(),
        facts: Facts::default(),
    }));
    assert_eq!(app.records.len(), 2);
    // A refresh spawned after coming back still applies.
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 6,
        records: vec![record(1)],
        facts: Facts::default(),
    }));
    assert_eq!(app.records.len(), 1);
}

/// Open the pane on the first model, type `hi`, send it; the ask's number.
fn ask(app: &mut App) -> u64 {
    press(app, Key::Char('t'));
    for c in "hi".chars() {
        press(app, Key::Char(c));
    }
    match press(app, Key::Enter).as_slice() {
        [Effect::Ask { generation, .. }] => *generation,
        other => panic!("expected an ask, got {other:?}"),
    }
}

fn reply(app: &mut App, generation: u64, step: ReplyStep) -> Vec<Effect> {
    app.reduce(Event::Reply(Reply { generation, step }))
}

#[test]
fn try_opens_the_chat_pane_and_enter_asks() {
    let mut app = app(1);
    press(&mut app, Key::Char('t'));
    assert!(matches!(app.modal, Some(Modal::Chat(_))));
    assert!(press(&mut app, Key::Enter).is_empty());
    press(&mut app, Key::Escape);
    assert!(ask(&mut app) > 0);
    assert!(press(&mut app, Key::Char('q')).is_empty());
    assert!(matches!(app.modal, Some(Modal::Chat(_))));
}

#[test]
fn a_streamed_reply_lands_in_the_pane_and_refreshes_when_done() {
    let mut app = app(1);
    let generation = ask(&mut app);
    app.take_dirty();
    let effects = reply(&mut app, generation, ReplyStep::Text("yo".to_owned()));
    assert!(effects.is_empty() && app.take_dirty());
    let effects = reply(&mut app, generation, ReplyStep::Done(None));
    assert_eq!(effects, vec![Effect::Refresh]);
    let pane = app.chat_pane().expect("the pane");
    assert_eq!(pane.turns.last().map(|turn| turn.text.as_str()), Some("yo"));
    assert!(!pane.streaming());
}

#[test]
fn escape_stops_a_reply_first_and_closes_the_pane_second() {
    let mut app = app(1);
    let generation = ask(&mut app);
    assert_eq!(press(&mut app, Key::Escape), vec![Effect::StopAsk]);
    app.take_dirty();
    let effects = reply(&mut app, generation, ReplyStep::Text("late".to_owned()));
    assert!(effects.is_empty() && !app.take_dirty());
    assert!(press(&mut app, Key::Escape).is_empty());
    assert!(app.modal.is_none());
    assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
}

#[test]
fn ctrl_c_closes_an_idle_chat_pane_and_quits_from_every_other_modal() {
    let mut app = app(1);
    press(&mut app, Key::Char('t'));
    assert!(press(&mut app, Key::Interrupt).is_empty());
    assert!(app.modal.is_none());
    for open in ['p', 'x', 'l', '?'] {
        press(&mut app, Key::Char(open));
        assert!(app.modal.is_some(), "{open} opens a modal");
        assert_eq!(press(&mut app, Key::Interrupt), vec![Effect::Quit]);
        app.modal = None;
    }
}

#[test]
fn a_reopened_pane_never_takes_the_closed_ones_reply() {
    let mut app = app(1);
    let first = ask(&mut app);
    press(&mut app, Key::Escape);
    press(&mut app, Key::Escape);
    let second = ask(&mut app);
    assert!(second > first);
    reply(&mut app, first, ReplyStep::Text("stale".to_owned()));
    let pane = app.chat_pane().expect("the pane");
    assert_eq!(pane.turns.last().map(|turn| turn.text.as_str()), Some(""));
}

#[test]
fn chat_and_serve_hand_off_when_they_can() {
    let mut app = app(1);
    assert!(matches!(
        press(&mut app, Key::Char('T')).as_slice(),
        [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Chat { .. })
    ));
    assert!(matches!(
        press(&mut app, Key::Char('S')).as_slice(),
        [Effect::HandOff(hand_off)] if matches!(**hand_off, HandOff::Serve)
    ));
    app.facts.gateway_port = Some(4321);
    assert!(press(&mut app, Key::Char('S')).is_empty());
    assert_eq!(app.notice(), Some("the gateway is already on :4321"));
}

#[test]
fn a_finished_pull_selects_what_it_pulled_on_the_next_refresh() {
    let mut app = app(2);
    app.reduce(polled(
        "qwen2.5:14b",
        PullState::Queued,
        TaskState::Status("queued".to_owned()),
    ));
    let effects = app.reduce(polled(
        "qwen2.5:14b",
        PullState::Done,
        TaskState::Done("pulled qwen2.5:14b".to_owned()),
    ));
    assert_eq!(effects, vec![Effect::Refresh]);
    // A refresh that predates the pulled record leaves the intent alone.
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 8,
        records: app.records.clone(),
        facts: Facts::default(),
    }));
    let mut records = app.records.clone();
    records.push(ModelRecord::new(
        "qwen2.5:14b",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), "qwen2.5:14b"),
    ));
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 9,
        records,
        facts: Facts::default(),
    }));
    assert_eq!(
        app.selected_record().map(|r| r.name.as_str()),
        Some("qwen2.5:14b")
    );
}

#[test]
fn actions_follow_the_selected_model() {
    let mut one = app(1);
    let id = one.records[0].id.clone();
    assert_eq!(one.actions(), vec!["w", "l", "t", "T", "x"]);
    one.facts.residents.push(resident(&id, Holder::Daemon));
    assert_eq!(one.actions(), vec!["u", "l", "t", "T"]);
    one.facts.residents[0].holder = Holder::Gateway;
    assert_eq!(one.actions(), vec!["l", "t", "T"]);
    one.facts.residents.clear();
    one.records[0].capabilities = vec![Capability::speak()];
    one.records[0].primary_weight_path = Some("/w".to_owned());
    one.reorder_in_place();
    assert_eq!(one.actions(), vec!["w", "x", "y"]);
    for key in one.actions() {
        assert!(keymap::binding(key).is_some(), "{key} is not bound");
    }
    let empty = app(0);
    assert!(empty.actions().is_empty());
}

/// The keys a binding names, as the reducer receives them.
fn keys_of(binding: &keymap::Binding) -> Vec<Key> {
    match binding.key {
        "enter" => vec![Key::Enter],
        "esc" => vec![Key::Escape],
        "↑/↓" => vec![Key::Up, Key::Down],
        key => keymap::chars(key).into_iter().map(Key::Char).collect(),
    }
}

#[test]
fn every_binding_does_something() {
    for binding in keymap::BINDINGS {
        for key in keys_of(binding) {
            // Selected in the middle of three, so each move key has
            // somewhere to go; expanded, so escape has something to
            // collapse.
            let mut app = app(3);
            press(&mut app, Key::Down);
            if binding.key == "esc" {
                press(&mut app, Key::Enter);
            }
            app.take_dirty();
            let effects = press(&mut app, key);
            assert!(
                !effects.is_empty() || app.take_dirty() || app.notice().is_some(),
                "{} ({key:?}) does nothing",
                binding.key
            );
        }
    }
}

#[test]
fn every_unbound_char_does_nothing() {
    let bound: Vec<char> = keymap::BINDINGS
        .iter()
        .flat_map(|binding| keymap::chars(binding.key))
        .collect();
    for c in (0x20u8..=0x7e)
        .map(char::from)
        .filter(|c| !bound.contains(c))
    {
        let mut app = app(3);
        press(&mut app, Key::Down);
        app.take_dirty();
        let effects = press(&mut app, Key::Char(c));
        assert!(effects.is_empty(), "{c:?} has effects but is not bound");
        assert!(!app.take_dirty(), "{c:?} redraws but is not bound");
        assert!(app.notice().is_none(), "{c:?} notifies but is not bound");
    }
}

#[test]
fn an_older_refresh_never_overwrites_a_newer_one() {
    let mut app = app(1);
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 5,
        records: vec![record(5)],
        facts: Facts::default(),
    }));
    app.reduce(Event::Refreshed(Refreshed {
        sequence: 4,
        records: vec![record(4)],
        facts: Facts::default(),
    }));
    assert_eq!(app.records[0].name, "model-5");
}
