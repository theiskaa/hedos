//! Integration tests for the `registry` store: CRUD, change detection,
//! case-insensitive listing, state updates, transform-based updates, persistence
//! across reopen, and corruption recovery. Public API only.

mod support;

use std::fs;

use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, ModelState, ParamSpec, ParamType,
    SourceKind, stable_id,
};
use kernel::registry::{Registry, RegistryError};
use support::TempDir;

fn record(name: &str, path: &str) -> ModelRecord {
    ModelRecord::new(
        name,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), path),
    )
}

#[test]
fn open_empty_directory_is_empty() {
    let dir = TempDir::new();
    let registry = Registry::open(dir.path()).unwrap();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.list().is_empty());
}

#[test]
fn register_persists_and_survives_reopen() {
    let dir = TempDir::new();
    let rec = record("Model", "/m.gguf");
    let id = rec.id.clone();
    {
        let mut registry = Registry::open(dir.path()).unwrap();
        assert!(registry.register(rec).unwrap());
        assert!(registry.contains(&id));
    }
    let reopened = Registry::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&id).unwrap().name, "Model");
    assert_eq!(reopened.len(), 1);
}

#[test]
fn registering_an_identical_record_is_a_noop() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let rec = record("Model", "/m.gguf");
    assert!(
        registry.register(rec.clone()).unwrap(),
        "first insert changes"
    );
    assert!(
        !registry.register(rec).unwrap(),
        "identical re-insert is a no-op"
    );
}

#[test]
fn register_all_counts_changes_and_is_idempotent() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let records = vec![record("A", "/a.gguf"), record("B", "/b.gguf")];
    assert_eq!(registry.register_all(records.clone()).unwrap(), 2);
    assert_eq!(registry.len(), 2);
    assert_eq!(
        registry.register_all(records).unwrap(),
        0,
        "no changes second time"
    );
}

#[test]
fn unregister_removes_and_persists() {
    let dir = TempDir::new();
    let rec = record("Model", "/m.gguf");
    let id = rec.id.clone();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec).unwrap();

    let removed = registry.unregister(&id).unwrap();
    assert_eq!(removed.unwrap().id, id);
    assert!(!registry.contains(&id));
    assert!(
        registry.unregister(&id).unwrap().is_none(),
        "removing again is None"
    );

    let reopened = Registry::open(dir.path()).unwrap();
    assert!(reopened.get(&id).is_none());
}

#[test]
fn list_is_sorted_case_insensitively() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(record("banana", "/1")).unwrap();
    registry.register(record("Apple", "/2")).unwrap();
    registry.register(record("Cherry", "/3")).unwrap();
    let names: Vec<&str> = registry.list().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Apple", "banana", "Cherry"]);
}

#[test]
fn set_state_updates_present_and_ignores_absent() {
    let dir = TempDir::new();
    let rec = record("Model", "/m.gguf");
    let id = rec.id.clone();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec).unwrap();

    assert!(
        registry
            .set_state_if_present(&id, ModelState::Ready)
            .unwrap()
    );
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Ready);
    assert!(
        !registry
            .set_state_if_present("nonexistent", ModelState::Missing)
            .unwrap()
    );

    let reopened = Registry::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&id).unwrap().state, ModelState::Ready);
}

#[test]
fn update_applies_transform_and_skips_none() {
    let dir = TempDir::new();
    let rec = record("Model", "/m.gguf");
    let id = rec.id.clone();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec).unwrap();

    let changed = registry
        .update(std::slice::from_ref(&id), |current| {
            let mut next = current.clone();
            next.alias = Some("Nick".into());
            Some(next)
        })
        .unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(registry.get(&id).unwrap().alias.as_deref(), Some("Nick"));

    let unchanged = registry
        .update(std::slice::from_ref(&id), |_| None)
        .unwrap();
    assert!(unchanged.is_empty(), "returning None changes nothing");

    let same = registry
        .update(&[id], |current| Some(current.clone()))
        .unwrap();
    assert!(
        same.is_empty(),
        "an identical transform result changes nothing"
    );

    let absent = registry
        .update(&["missing".to_owned()], |c| Some(c.clone()))
        .unwrap();
    assert!(absent.is_empty());
}

#[test]
fn corrupt_store_is_quarantined_and_reported() {
    let dir = TempDir::new();
    fs::write(dir.join("models.json"), b"{ not valid json").unwrap();

    match Registry::open(dir.path()) {
        Err(RegistryError::CorruptStore(_)) => {}
        other => panic!("expected CorruptStore, got {other:?}"),
    }
    assert!(
        !dir.join("models.json").exists(),
        "corrupt store should be quarantined"
    );
    let quarantined = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("models.json.corrupt-")
        });
    assert!(quarantined);
}

#[test]
fn update_that_changes_id_migrates_the_key() {
    let dir = TempDir::new();
    let rec = record("Model", "/a.gguf");
    let old_id = rec.id.clone();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec).unwrap();

    let changed = registry
        .update(std::slice::from_ref(&old_id), |current| {
            let mut next = current.clone();
            next.source = ModelSource::new(SourceKind::file(), "/b.gguf");
            next.id = stable_id(&next.source);
            Some(next)
        })
        .unwrap();
    let new_id = changed[0].id.clone();
    assert_ne!(new_id, old_id);
    assert!(registry.get(&old_id).is_none(), "old key must be gone");
    assert_eq!(registry.get(&new_id).unwrap().name, "Model");
    assert_eq!(registry.len(), 1);

    let reopened = Registry::open(dir.path()).unwrap();
    assert!(reopened.get(&old_id).is_none());
    assert_eq!(reopened.get(&new_id).unwrap().id, new_id);
}

#[test]
fn register_replaces_existing_content_and_persists() {
    let dir = TempDir::new();
    let mut rec = record("Model", "/m.gguf");
    let id = rec.id.clone();
    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec.clone()).unwrap();

    rec.alias = Some("Renamed".into());
    assert!(
        registry.register(rec).unwrap(),
        "changed content is a change"
    );
    assert_eq!(registry.get(&id).unwrap().alias.as_deref(), Some("Renamed"));

    let reopened = Registry::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&id).unwrap().alias.as_deref(), Some("Renamed"));
}

#[test]
fn reopen_preserves_full_record_fidelity() {
    let dir = TempDir::new();
    let mut rec = record("Model", "/m.gguf");
    rec.alias = Some("Nick".into());
    rec.system_prompt = Some("be terse".into());
    rec.footprint_mb = Some(4096);
    rec.content_fingerprint = Some("abcd".into());
    rec.params.push(ParamSpec {
        key: "temperature".into(),
        param_type: ParamType::Float,
        default_value: Some(JsonValue::Double(0.8)),
        range: Some(vec![JsonValue::Double(0.0), JsonValue::Double(2.0)]),
        values: None,
    });
    rec.param_values
        .insert("temperature".into(), JsonValue::Double(0.5));
    let id = rec.id.clone();

    let mut registry = Registry::open(dir.path()).unwrap();
    registry.register(rec.clone()).unwrap();
    let reopened = Registry::open(dir.path()).unwrap();
    assert_eq!(reopened.get(&id), Some(&rec));
}

#[test]
fn future_schema_is_rejected_not_downgraded() {
    let dir = TempDir::new();
    let file = dir.join("models.json");
    fs::write(&file, br#"{"schema_version":999,"models":[]}"#).unwrap();

    match Registry::open(dir.path()) {
        Err(RegistryError::FutureSchema {
            found: 999,
            supported: 1,
        }) => {}
        other => panic!("expected FutureSchema, got {other:?}"),
    }
    assert!(
        file.exists(),
        "a future-schema store must be left in place, not touched"
    );
}

#[test]
fn duplicate_ids_in_file_keep_the_last() {
    let dir = TempDir::new();
    fs::write(
        dir.join("models.json"),
        br#"{"schema_version":1,"models":[
            {"id":"dup","name":"First","modality":"text","capabilities":["chat"],
             "source":{"kind":"file","path":"/a"},"registered_at":1},
            {"id":"dup","name":"Second","modality":"text","capabilities":["chat"],
             "source":{"kind":"file","path":"/b"},"registered_at":2}
        ]}"#,
    )
    .unwrap();

    let registry = Registry::open(dir.path()).unwrap();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.get("dup").unwrap().name, "Second");
}

#[test]
fn register_all_with_internal_duplicate_keeps_last() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let mut first = record("First", "/x.gguf");
    let mut second = first.clone();
    second.name = "Second".into();
    assert_eq!(second.id, first.id, "same source means same id");
    first.name = "First".into();

    registry.register_all(vec![first, second]).unwrap();
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.list()[0].name, "Second");
}

#[test]
fn register_all_counts_only_new_or_changed() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let a = record("A", "/a.gguf");
    let b = record("B", "/b.gguf");
    registry.register(a.clone()).unwrap();

    let c = record("C", "/c.gguf");
    assert_eq!(
        registry.register_all(vec![a, b, c]).unwrap(),
        2,
        "a is unchanged"
    );
    assert_eq!(registry.len(), 3);
}

#[test]
fn list_tie_breaks_by_id_when_names_match_case_insensitively() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let lower = record("apple", "/one");
    let upper = record("Apple", "/two");
    let mut ids = [lower.id.clone(), upper.id.clone()];
    ids.sort();
    registry.register(lower).unwrap();
    registry.register(upper).unwrap();

    let listed: Vec<&str> = registry.list().iter().map(|r| r.id.as_str()).collect();
    assert_eq!(listed, [ids[0].as_str(), ids[1].as_str()]);
}

#[test]
fn update_over_multiple_ids_changes_only_the_relevant_ones() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    let a = record("A", "/a");
    let b = record("B", "/b");
    let c = record("C", "/c");
    let (ida, idb, idc) = (a.id.clone(), b.id.clone(), c.id.clone());
    registry.register_all(vec![a, b, c]).unwrap();

    let changed = registry
        .update(&[ida.clone(), idb.clone(), idc.clone()], |current| {
            if current.id == idb {
                let mut next = current.clone();
                next.alias = Some("only-b".into());
                Some(next)
            } else if current.id == idc {
                None
            } else {
                Some(current.clone())
            }
        })
        .unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].id, idb);
    assert_eq!(registry.get(&idb).unwrap().alias.as_deref(), Some("only-b"));
    assert!(registry.get(&ida).unwrap().alias.is_none());
}

#[test]
fn unregister_absent_id_returns_none() {
    let dir = TempDir::new();
    let mut registry = Registry::open(dir.path()).unwrap();
    assert!(registry.unregister("never").unwrap().is_none());
}
