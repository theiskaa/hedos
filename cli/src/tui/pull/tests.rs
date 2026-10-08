use super::*;
use crate::tui::testing;
use kernel::records::{Capability, Modality, ModelSource, SourceKind};

const MEMORY: u64 = 64 << 30;

fn hit(reference: &str) -> InstallSearchHit {
    InstallSearchHit {
        provider: InstallProviderId::huggingface(),
        reference: reference.to_owned(),
        name: reference.to_owned(),
        downloads: Some(10),
        likes: None,
        updated_at: None,
    }
}

fn shelf_record(reference: &str) -> ModelRecord {
    ModelRecord::new(
        reference,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), reference),
    )
}

fn type_in(modal: &mut PullModal, text: &str) {
    for c in text.chars() {
        modal.edit(Key::Char(c), 0);
    }
}

/// A gigabyte-sized Ollama plan for `reference`, gated when `requires_auth`.
fn sized_plan(reference: &str, requires_auth: bool) -> InstallPlan {
    InstallPlan {
        total_bytes: Some(1 << 30),
        remaining_bytes: Some(1 << 30),
        destination: "~/.ollama".to_owned(),
        requires_auth,
        ..testing::plan(reference)
    }
}

#[test]
fn a_blank_query_offers_recommendations_of_every_kind() {
    let modal = PullModal::open(&[], MEMORY, &[]);
    assert!(!modal.matches.is_empty());
    assert!(modal.matches.iter().all(|m| m.bytes.is_some()));
    assert_eq!(modal.count(Kind::All), modal.matches.len());
    let kinds = KINDS[1..]
        .iter()
        .filter(|kind| modal.count(**kind) > 0)
        .count();
    assert!(kinds > 1);
}

#[test]
fn a_model_on_the_shelf_is_listed_muted_and_a_gone_one_offered_again() {
    let first = PullModal::open(&[], MEMORY, &[]).matches[0].clone();
    let mut gone = shelf_record(&first.reference);
    let modal = PullModal::open(&[shelf_record(&first.reference)], MEMORY, &[]);
    let row = modal
        .matches
        .iter()
        .find(|m| m.reference == first.reference)
        .expect("listed");
    assert_eq!(row.shelf, Some(OnShelf::Present));
    gone.state = ModelState::Missing;
    let modal = PullModal::open(&[gone], MEMORY, &[]);
    let row = modal
        .matches
        .iter()
        .find(|m| m.reference == first.reference)
        .expect("listed");
    assert_eq!(row.shelf, Some(OnShelf::Gone));
}

#[test]
fn enter_refuses_what_is_on_the_shelf_or_downloading() {
    let first = PullModal::open(&[], MEMORY, &[]).matches[0]
        .reference
        .clone();
    let mut modal = PullModal::open(&[shelf_record(&first)], MEMORY, &[]);
    assert!(modal.enter().unwrap_err().contains("already on the shelf"));
    let mut modal = PullModal::open(&[], MEMORY, std::slice::from_ref(&first));
    assert!(modal.matches[0].pulling);
    assert!(modal.enter().unwrap_err().contains("already downloading"));
}

#[test]
fn a_typed_reference_leads_the_list_and_a_search_falls_due() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    type_in(&mut modal, "Qwen/Qwen2.5-14B");
    assert_eq!(modal.matches[0].reference, "Qwen/Qwen2.5-14B");
    assert_eq!(modal.matches[0].note, "as typed");
    assert_eq!(modal.search_due(1), None);
    assert_eq!(
        modal.search_due(SEARCH_DEBOUNCE_TICKS),
        Some("Qwen/Qwen2.5-14B".to_owned())
    );
    assert_eq!(*modal.search(), Search::Asked);
    assert_eq!(modal.search_due(SEARCH_DEBOUNCE_TICKS), None);
}

#[test]
fn hits_apply_only_to_the_current_query_and_a_failure_is_kept() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    type_in(&mut modal, "smol");
    modal.searched("stale", &[hit("x/stale")], None);
    assert!(modal.matches.iter().all(|m| m.reference != "x/stale"));
    modal.searched("smol", &[hit("x/smol-1")], None);
    assert!(modal.matches.iter().any(|m| m.reference == "x/smol-1"));
    assert_eq!(modal.matches.last().and_then(|m| m.downloads), Some(10));
    assert_eq!(*modal.search(), Search::Done);
    modal.edit(Key::Backspace, 5);
    assert!(modal.matches.iter().all(|m| m.reference != "x/smol-1"));
    modal.edit(Key::Char('l'), 5);
    modal.searched("smol", &[], Some("hugging face is unreachable".to_owned()));
    assert!(matches!(modal.search(), Search::Failed(note) if note.contains("unreachable")));
}

#[test]
fn a_kind_narrows_the_list_and_search_hits_show_only_under_all() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    modal.searched("", &[hit("x/anything")], None);
    let all = modal.matches.len();
    modal.cycle_kind(1, 0, 0);
    assert_eq!(modal.kind(), KINDS[1]);
    assert!(modal.matches.len() < all);
    assert!(
        modal
            .matches
            .iter()
            .all(|m| m.category == Some(InstallCategory::Chat))
    );
    assert_eq!(modal.count(KINDS[1]), modal.matches.len());
    modal.cycle_kind(-1, 0, 0);
    modal.cycle_kind(-1, 0, 0);
    assert_eq!(modal.kind(), KINDS[KINDS.len() - 1]);
}

#[test]
fn a_rested_row_is_planned_once_and_scrolling_past_asks_nothing() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    assert_eq!(modal.plan_due(PLAN_SETTLE_TICKS - 1), None);
    let (provider, reference, ask) = modal.plan_due(PLAN_SETTLE_TICKS).expect("due");
    assert!(modal.planning());
    assert_eq!(modal.plan_due(PLAN_SETTLE_TICKS + 5), None, "asked once");
    modal.step(1, 10, 0);
    assert_eq!(modal.plan_due(11), None, "still moving");
    let started = modal.planned(
        &provider,
        &reference,
        ask,
        Ok(sized_plan(&reference, false)),
    );
    assert_eq!(started, None, "nothing was armed");
    modal.step(-1, 12, 0);
    assert!(matches!(
        modal.selected_offer().and_then(|o| modal.plan(o)),
        Some(Plan::Ready(_))
    ));
    assert_eq!(modal.plan_due(20), None, "a ready plan asks nothing again");
}

#[test]
fn an_answer_to_an_older_ask_never_lands() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let (provider, reference, ask) = modal.plan_due(PLAN_SETTLE_TICKS).expect("due");
    modal.planned(&provider, &reference, ask + 1000, Err("stale".to_owned()));
    assert!(modal.planning());
    modal.planned(&provider, &reference, ask, Ok(sized_plan(&reference, true)));
    assert_eq!(
        modal.selected_offer().and_then(|o| modal.plan(o)),
        Some(&Plan::Gated)
    );
    assert!(modal.enter().unwrap_err().contains("gated"));
}

#[test]
fn a_failed_plan_is_asked_again_only_after_leaving_the_row() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let (provider, reference, ask) = modal.plan_due(PLAN_SETTLE_TICKS).expect("due");
    modal.planned(&provider, &reference, ask, Err("no network".to_owned()));
    assert_eq!(modal.plan_due(50), None);
    assert_eq!(modal.enter().unwrap_err(), "no network");
    modal.step(1, 60, 0);
    modal.step(-1, 61, 0);
    assert!(modal.plan_due(61 + PLAN_SETTLE_TICKS).is_some());
}

#[test]
fn enter_before_the_plan_arms_the_row_and_its_plan_starts_it() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let Ok(Enter::Plan(provider, reference, ask)) = modal.enter() else {
        panic!("enter on an unplanned row asks for its plan");
    };
    let offer = modal.selected_offer().expect("a row").clone();
    assert!(modal.armed(&offer));
    let other = modal.planned(&provider, "someone/else", ask, Ok(sized_plan("x", false)));
    assert_eq!(other, None);
    let started = modal.planned(
        &provider,
        &reference,
        ask,
        Ok(sized_plan(&reference, false)),
    );
    assert_eq!(
        started.and_then(Result::ok).map(|plan| plan.reference),
        Some(reference)
    );
    assert!(!modal.armed(&offer));
}

#[test]
fn leaving_an_armed_row_lets_go_of_its_enter() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let Ok(Enter::Plan(provider, reference, ask)) = modal.enter() else {
        panic!("enter on an unplanned row asks for its plan");
    };
    modal.step(1, 10, 0);
    let started = modal.planned(
        &provider,
        &reference,
        ask,
        Ok(sized_plan(&reference, false)),
    );
    assert_eq!(started, None, "the cursor left the row");
}

#[test]
fn an_armed_row_whose_plan_is_too_big_is_refused_not_started() {
    let mut modal = PullModal::open(&[], 1 << 30, &[]);
    type_in(&mut modal, "someone/huge-70b");
    let Ok(Enter::Plan(provider, reference, ask)) = modal.enter() else {
        panic!("enter on an unplanned row asks for its plan");
    };
    let mut plan = sized_plan(&reference, false);
    plan.total_bytes = Some(8 << 30);
    let started = modal.planned(&provider, &reference, ask, Ok(plan));
    assert!(
        matches!(&started, Some(Err(reason)) if reason.contains("this machine has")),
        "{started:?}"
    );
}

#[test]
fn a_reference_being_typed_is_planned_once_its_search_settles() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    type_in(&mut modal, "Qwen/Qwen2.5-7");
    assert_eq!(modal.plan_due(PLAN_SETTLE_TICKS), None);
    modal.search_due(SEARCH_DEBOUNCE_TICKS);
    modal.searched("Qwen/Qwen2.5-7", &[], None);
    assert!(modal.plan_due(SEARCH_DEBOUNCE_TICKS).is_some());
}

#[test]
fn enter_on_a_ready_row_starts_it() {
    let plan = sized_plan("gemma3", false);
    let mut modal = PullModal::ready(plan.clone());
    assert_eq!(modal.enter(), Ok(Enter::Start(Box::new(plan))));
}

#[test]
fn a_bare_word_is_a_search_not_a_tag() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    type_in(&mut modal, "smol");
    assert!(modal.matches.iter().all(|m| m.note != "as typed"));
    type_in(&mut modal, ":latest");
    assert_eq!(modal.matches[0].reference, "smol:latest");
}

#[test]
fn repeats_and_overflow_never_hide_search_hits() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let first = modal.matches[0].reference.clone();
    let mut hits: Vec<InstallSearchHit> = (0..SEARCH_LIMIT)
        .map(|index| hit(&format!("x/hit-{index}")))
        .collect();
    hits[0].reference = first.to_uppercase();
    hits[0].provider = modal.matches[0].provider.clone();
    modal.searched("", &hits, None);
    let repeats = modal
        .matches
        .iter()
        .filter(|m| m.reference.eq_ignore_ascii_case(&first))
        .count();
    assert_eq!(repeats, 1);
    assert!(modal.matches.len() <= MAX_MATCHES);
    assert!(modal.matches.iter().any(|m| m.reference == "x/hit-7"));
}

#[test]
fn a_blank_query_is_grouped_by_kind_in_order() {
    let modal = PullModal::open(&[], MEMORY, &[]);
    let categories: Vec<InstallCategory> =
        modal.matches.iter().filter_map(|m| m.category).collect();
    let order: Vec<usize> = categories
        .iter()
        .filter_map(|c| CATEGORIES.iter().position(|k| k == c))
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(order, sorted);
    let headings = modal
        .rows()
        .iter()
        .filter(|row| matches!(row, ListingRow::Eyebrow(_)))
        .count();
    assert!(headings > 1);
}

#[test]
fn stepping_clamps_and_esc_clears_the_query() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    modal.step(-3, 0, 0);
    assert_eq!(modal.selected, 0);
    modal.step(100, 0, 7);
    assert_eq!(modal.selected, modal.matches.len() - 1);
    assert_eq!(modal.selected_at, 7);
    type_in(&mut modal, "qwen");
    modal.clear_query(3);
    assert!(modal.input.is_empty());
    assert_eq!(*modal.search(), Search::Idle);
}

#[test]
fn a_too_big_model_is_refused_with_its_size() {
    let mut modal = PullModal::open(&[], 1 << 30, &[]);
    let big = modal
        .matches
        .iter()
        .position(|offer| offer.fit(1 << 30) == Some(FitVerdict::TooLarge));
    if let Some(index) = big {
        modal.selected = index;
        assert!(modal.enter().unwrap_err().contains("this machine has"));
        assert_eq!(modal.plan_due(100), None, "nothing to plan");
    }
}
