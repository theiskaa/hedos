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

#[test]
fn a_blank_query_offers_recommendations() {
    let modal = PullModal::open(&[], MEMORY, &[]);
    assert!(!modal.matches.is_empty());
    assert!(modal.matches.iter().all(|m| m.bytes.is_some()));
    assert_eq!(fit(Some(4 << 30), MEMORY), Some(FitVerdict::RunsWell));
}

#[test]
fn installed_models_are_left_out() {
    let first = PullModal::open(&[], MEMORY, &[]).matches[0].clone();
    let record = ModelRecord::new(
        &first.reference,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), &first.reference),
    );
    let mut modal = PullModal::open(&[record], MEMORY, &[]);
    assert!(modal.matches.iter().all(|m| m.reference != first.reference));
    assert!(modal.direct_installed.is_none());
    for c in first.reference.chars() {
        modal.edit(Key::Char(c), 0);
    }
    assert!(modal.direct_installed.is_some());
    assert!(modal.matches.iter().all(|m| m.reference != first.reference));
    modal.edit(Key::Backspace, 0);
    assert!(modal.direct_installed.is_none());
}

#[test]
fn a_typed_reference_leads_the_list_and_a_search_falls_due() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    for c in "Qwen/Qwen2.5-14B".chars() {
        modal.edit(Key::Char(c), 0);
    }
    assert_eq!(modal.matches[0].reference, "Qwen/Qwen2.5-14B");
    assert_eq!(modal.matches[0].note, "as typed");
    assert_eq!(modal.search_due(1), None);
    assert_eq!(
        modal.search_due(SEARCH_DEBOUNCE_TICKS),
        Some("Qwen/Qwen2.5-14B".to_owned())
    );
    assert_eq!(modal.search_due(SEARCH_DEBOUNCE_TICKS), None);
}

#[test]
fn hits_apply_only_to_the_current_query() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    for c in "smol".chars() {
        modal.edit(Key::Char(c), 0);
    }
    modal.searched("stale", &[hit("x/stale")]);
    assert!(modal.matches.iter().all(|m| m.reference != "x/stale"));
    modal.searched("smol", &[hit("x/smol-1")]);
    assert!(modal.matches.iter().any(|m| m.reference == "x/smol-1"));
    modal.edit(Key::Backspace, 5);
    assert!(modal.matches.iter().all(|m| m.reference != "x/smol-1"));
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
fn choosing_plans_and_the_plan_moves_to_a_preview() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let (_, reference, ask) = modal.choose().expect("a match");
    assert_eq!(modal.stage, Stage::Planning(reference.clone()));
    modal.planned(ask - 1, Err("ignored".to_owned()));
    assert_eq!(modal.stage, Stage::Planning(reference.clone()));
    modal.back();
    let (_, _, again) = modal.choose().expect("a match");
    modal.planned(ask, Err("stale".to_owned()));
    assert_eq!(modal.stage, Stage::Planning(reference.clone()));
    modal.planned(again, Ok(sized_plan(&reference, false)));
    assert_eq!(modal.stage, Stage::Preview(sized_plan(&reference, false)));
    modal.back();
    assert_eq!(modal.stage, Stage::Listing);
}

#[test]
fn a_gated_plan_becomes_a_note() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    let (_, reference, ask) = modal.choose().expect("a match");
    modal.planned(ask, Ok(sized_plan(&reference, true)));
    assert!(matches!(modal.stage, Stage::Note(ref note) if note.contains("gated")));
}

#[test]
fn a_bare_word_is_a_search_not_a_tag() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    for c in "smol".chars() {
        modal.edit(Key::Char(c), 0);
    }
    assert!(modal.matches.iter().all(|m| m.note != "as typed"));
    for c in ":latest".chars() {
        modal.edit(Key::Char(c), 0);
    }
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
    modal.searched("", &hits);
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
fn a_blank_query_is_grouped_by_category_in_order() {
    let modal = PullModal::open(&[], MEMORY, &[]);
    let categories: Vec<InstallCategory> =
        modal.matches.iter().filter_map(|m| m.category).collect();
    let mut order: Vec<usize> = categories
        .iter()
        .map(|c| CATEGORIES.iter().position(|k| k == c).unwrap())
        .collect();
    let sorted = {
        let mut s = order.clone();
        s.sort_unstable();
        s
    };
    assert_eq!(order, sorted);
    order.dedup();
    assert!(order.len() > 1);
    for category in CATEGORIES {
        assert!(
            modal
                .matches
                .iter()
                .filter(|m| m.category == Some(category))
                .count()
                <= 3
        );
    }
}

#[test]
fn an_in_flight_pull_is_listed_but_not_choosable() {
    let first = PullModal::open(&[], MEMORY, &[]).matches[0]
        .reference
        .clone();
    let mut modal = PullModal::open(&[], MEMORY, std::slice::from_ref(&first));
    assert!(modal.matches[0].pulling);
    assert!(modal.choose().unwrap_err().contains("already downloading"));
    assert_eq!(modal.stage, Stage::Listing);
}

#[test]
fn stepping_clamps() {
    let mut modal = PullModal::open(&[], MEMORY, &[]);
    modal.step(-3);
    assert_eq!(modal.selected, 0);
    modal.step(100);
    assert_eq!(modal.selected, modal.matches.len() - 1);
}
