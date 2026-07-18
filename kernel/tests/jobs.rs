//! Tests for the jobs data layer: seed injection, the `Job` model, and the
//! terminal-job history store.

mod support;

use kernel::jobs::{Job, JobHistoryStore, JobState, reseeded, seeded};
use kernel::records::{Capability, JsonValue};
use support::TempDir;

fn obj<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn seed_of(value: &JsonValue) -> Option<&JsonValue> {
    value.as_object().and_then(|obj| obj.get("seed"))
}

#[test]
fn seeding_injects_a_seed_only_when_absent_or_null() {
    let bare = seeded(&obj([("prompt", JsonValue::String("hi".to_owned()))]));
    assert!(matches!(seed_of(&bare), Some(JsonValue::Int(_))));

    let null = seeded(&obj([("seed", JsonValue::Null)]));
    assert!(matches!(seed_of(&null), Some(JsonValue::Int(_))));

    let pinned = seeded(&obj([("seed", JsonValue::Int(42))]));
    assert_eq!(
        seed_of(&pinned),
        Some(&JsonValue::Int(42)),
        "a pinned seed is kept"
    );

    let scalar = seeded(&JsonValue::String("not an object".to_owned()));
    assert_eq!(
        scalar,
        JsonValue::String("not an object".to_owned()),
        "non-objects pass through"
    );

    let from_null = seeded(&JsonValue::Null);
    assert!(
        matches!(seed_of(&from_null), Some(JsonValue::Int(_))),
        "null becomes a seeded object"
    );
}

#[test]
fn reseeding_always_picks_a_different_seed_and_keeps_other_fields() {
    let params = obj([
        ("seed", JsonValue::Int(42)),
        ("prompt", JsonValue::String("hi".to_owned())),
    ]);
    for _ in 0..20 {
        let fresh = reseeded(&params);
        assert_ne!(
            seed_of(&fresh),
            Some(&JsonValue::Int(42)),
            "the seed changes"
        );
        assert_eq!(
            fresh.as_object().and_then(|obj| obj.get("prompt")),
            Some(&JsonValue::String("hi".to_owned())),
            "other fields are preserved"
        );
    }
}

#[test]
fn job_state_terminality() {
    assert!(JobState::Done.is_terminal());
    assert!(JobState::Failed.is_terminal());
    assert!(JobState::Cancelled.is_terminal());
    assert!(!JobState::Queued.is_terminal());
    assert!(!JobState::Preparing.is_terminal());
    assert!(!JobState::Running.is_terminal());
}

fn done_job(id: &str, submitted_at: i64) -> Job {
    let mut job = Job::new(
        id,
        "model",
        Capability::image(),
        JsonValue::Null,
        submitted_at,
    );
    job.state = JobState::Done;
    job
}

#[test]
fn history_records_newest_first_and_round_trips() {
    let dir = TempDir::new();
    let mut store = JobHistoryStore::with_default_limit(dir.path());
    store.record(done_job("a", 100)).expect("record a");
    store.record(done_job("b", 300)).expect("record b");
    store.record(done_job("c", 200)).expect("record c");

    let ids: Vec<_> = store.list().iter().map(|job| job.id.clone()).collect();
    assert_eq!(ids, vec!["b", "c", "a"], "newest submitted first");

    let mut reopened = JobHistoryStore::with_default_limit(dir.path());
    assert_eq!(reopened.list().len(), 3, "reloads from disk");
    assert_eq!(reopened.get("b").map(|job| job.submitted_at), Some(300));
    assert_eq!(reopened.get("missing"), None);
}

#[test]
fn recording_the_same_id_replaces_it_and_preview_is_never_persisted() {
    let dir = TempDir::new();
    let mut store = JobHistoryStore::with_default_limit(dir.path());
    let mut first = done_job("x", 100);
    first.preview = Some(vec![1, 2, 3]);
    first.result = vec!["art-1".to_owned()];
    first.error = Some("boom".to_owned());
    store.record(first).expect("record first");

    let mut updated = done_job("x", 100);
    updated.result = vec!["art-2".to_owned()];
    store.record(updated).expect("record updated");
    assert_eq!(store.list().len(), 1, "same id is replaced, not appended");

    let mut reopened = JobHistoryStore::with_default_limit(dir.path());
    let got = reopened.get("x").expect("job x");
    assert_eq!(got.result, vec!["art-2".to_owned()]);
    assert_eq!(
        got.preview, None,
        "preview bytes do not survive a round trip"
    );
}

#[test]
fn history_trims_to_the_limit_keeping_the_newest() {
    let dir = TempDir::new();
    let mut store = JobHistoryStore::new(dir.path(), 2);
    store.record(done_job("a", 100)).expect("a");
    store.record(done_job("b", 200)).expect("b");
    store.record(done_job("c", 300)).expect("c");

    let ids: Vec<_> = store.list().iter().map(|job| job.id.clone()).collect();
    assert_eq!(ids, vec!["c", "b"], "the oldest is trimmed");
}

#[test]
fn set_limit_floors_at_one() {
    let dir = TempDir::new();
    let mut store = JobHistoryStore::new(dir.path(), 5);
    store.set_limit(0);
    assert_eq!(store.limit(), 1);
}

#[test]
fn missing_and_corrupt_stores_load_empty() {
    let dir = TempDir::new();
    let mut missing = JobHistoryStore::with_default_limit(dir.path());
    assert!(missing.list().is_empty(), "a missing store is empty");

    std::fs::write(dir.join("jobs.json"), b"{ not valid json").expect("write corrupt");
    let mut corrupt = JobHistoryStore::with_default_limit(dir.path());
    assert!(corrupt.list().is_empty(), "a corrupt store starts empty");
    corrupt
        .record(done_job("z", 1))
        .expect("record after quarantine");
    assert_eq!(
        corrupt.list().len(),
        1,
        "the store is usable after quarantine"
    );
}
