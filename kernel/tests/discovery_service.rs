//! Tests for `DiscoveryService`: registry reconciliation (insert/update/missing),
//! the weights-present guard, moved-config migration, and the summary.

mod support;

use kernel::discovery::{
    DiscoveredModel, DiscoveryService, ScanResult, StoreScanner, content_fingerprint,
};
use kernel::records::{Capability, Modality, ModelRecord, ModelSource, ModelState, SourceKind};
use kernel::registry::Registry;
use support::TempDir;

/// A scanner that returns a fixed result.
struct Fake {
    kinds: Vec<SourceKind>,
    result: ScanResult,
}

impl StoreScanner for Fake {
    fn kinds(&self) -> Vec<SourceKind> {
        self.kinds.clone()
    }
    fn scan(&self) -> ScanResult {
        self.result.clone()
    }
}

fn scanner(kinds: Vec<SourceKind>, result: ScanResult) -> Box<dyn StoreScanner> {
    Box::new(Fake { kinds, result })
}

fn discovered(name: &str, kind: SourceKind, path: &str) -> DiscoveredModel {
    let mut model = DiscoveredModel::new(name, ModelSource::new(kind, path));
    model.modality_hint = Some(Modality::text());
    model.capabilities_hint = vec![Capability::chat()];
    model.footprint_bytes = 5;
    model
}

fn registry(dir: &TempDir) -> Registry {
    Registry::open(dir.path()).expect("registry")
}

fn record(name: &str, kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        name,
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(kind, path),
    )
}

#[test]
fn registers_new_discovered_models() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let result = ScanResult {
        discovered: vec![
            discovered("alpha", SourceKind::ollama(), "alpha"),
            discovered("beta", SourceKind::ollama(), "beta"),
        ],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);

    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(summary.total_count, 2);
    assert_eq!(summary.per_kind[&SourceKind::ollama()].count, 2);
    assert_eq!(registry.len(), 2);
    let alpha = registry
        .list()
        .into_iter()
        .find(|r| r.name == "alpha")
        .expect("alpha registered");
    assert_eq!(alpha.state, ModelState::Unresolved);
    assert_eq!(alpha.footprint_mb, Some(0));
}

#[test]
fn updates_an_existing_record_and_revives_a_missing_one() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    // Pre-register a record marked missing at the same source.
    let mut existing = record("old-name", SourceKind::ollama(), "same");
    existing.state = ModelState::Missing;
    let id = existing.id.clone();
    registry.register(existing).unwrap();

    let mut model = discovered("new-name", SourceKind::ollama(), "same");
    model.context_length_hint = Some(4096);
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);

    service.discover(&mut registry).expect("discover");
    let updated = registry.get(&id).expect("still there");
    assert_eq!(updated.name, "new-name");
    assert_eq!(updated.context_length, Some(4096));
    // A missing record found again is revived to unresolved.
    assert_eq!(updated.state, ModelState::Unresolved);
}

#[test]
fn marks_a_scanned_but_absent_model_missing() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    // A record whose weights are not on disk.
    let mut gone = record("gone", SourceKind::ollama(), "/nonexistent/path");
    gone.primary_weight_path = Some("/nonexistent/weight.gguf".to_owned());
    let id = gone.id.clone();
    registry.register(gone).unwrap();

    // The ollama store is scanned but returns nothing.
    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::ollama()],
        ScanResult::default(),
    )]);
    service.discover(&mut registry).expect("discover");
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Missing);
}

#[test]
fn a_present_weight_keeps_a_model_out_of_missing() {
    let dir = TempDir::new();
    let weight = dir.path().join("weight.gguf");
    std::fs::write(&weight, b"data").unwrap();
    let mut registry = registry(&dir);
    let mut present = record("here", SourceKind::ollama(), "/some/source");
    present.primary_weight_path = Some(weight.to_string_lossy().into_owned());
    let id = present.id.clone();
    registry.register(present).unwrap();

    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::ollama()],
        ScanResult::default(),
    )]);
    service.discover(&mut registry).expect("discover");
    // Weights present → not marked missing (stays unresolved).
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Unresolved);
}

#[test]
fn a_failed_kind_is_not_missing_swept_and_reports_an_issue() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let gone = record("gone", SourceKind::ollama(), "/nonexistent");
    let id = gone.id.clone();
    registry.register(gone).unwrap();

    let result = ScanResult {
        failed_kinds: vec![SourceKind::ollama()],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);
    let summary = service.discover(&mut registry).expect("discover");

    // The store failed, so the missing check is skipped for it.
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Unresolved);
    assert!(
        summary
            .issues
            .iter()
            .any(|issue| issue.contains("skipped the missing check for ollama"))
    );
    assert_eq!(summary.failed_kinds, vec![SourceKind::ollama()]);
}

#[test]
fn migrates_saved_config_from_a_moved_model() {
    let dir = TempDir::new();
    // The moved weight file (same content → same fingerprint at old and new).
    let weight = dir.path().join("moved.gguf");
    std::fs::write(&weight, b"identical-weights").unwrap();
    let print = content_fingerprint(&weight).expect("fingerprint");

    let mut registry = registry(&dir);
    // A missing record at the OLD location, carrying user config.
    let mut old = record("mymodel", SourceKind::file(), "/old/location.gguf");
    old.state = ModelState::Missing;
    old.content_fingerprint = Some(print);
    old.footprint_mb = Some(0);
    old.system_prompt = Some("be terse".to_owned());
    old.alias = Some("myfav".to_owned());
    let old_id = old.id.clone();
    registry.register(old).unwrap();

    // The model reappears at a NEW path (new id) with the same file content.
    let mut model = discovered("mymodel", SourceKind::file(), "/new/location.gguf");
    model.primary_weight_path = Some(weight.to_string_lossy().into_owned());
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::file()], result)]);
    service.discover(&mut registry).expect("discover");

    // The old orphan is gone; the new record inherited its config.
    assert!(registry.get(&old_id).is_none(), "orphan removed");
    let migrated = registry
        .list()
        .into_iter()
        .find(|r| r.name == "mymodel")
        .expect("new record");
    assert_ne!(migrated.id, old_id, "it is the new-location record");
    assert_eq!(migrated.system_prompt.as_deref(), Some("be terse"));
    assert_eq!(migrated.alias.as_deref(), Some("myfav"));
}

#[test]
fn summary_reports_duplicates() {
    let dir = TempDir::new();
    let a = dir.path().join("a.gguf");
    let b = dir.path().join("b.gguf");
    std::fs::write(&a, b"same-bytes").unwrap();
    std::fs::write(&b, b"same-bytes").unwrap();

    let mut model_a = discovered("a", SourceKind::file(), "/a");
    model_a.primary_weight_path = Some(a.to_string_lossy().into_owned());
    let mut model_b = discovered("b", SourceKind::file(), "/b");
    model_b.primary_weight_path = Some(b.to_string_lossy().into_owned());

    let result = ScanResult {
        discovered: vec![model_a, model_b],
        ..Default::default()
    };
    // Threshold 1 so the tiny files qualify.
    let service =
        DiscoveryService::with_threshold(vec![scanner(vec![SourceKind::file()], result)], 1);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(summary.duplicates.len(), 1);
    assert_eq!(summary.duplicates[0].paths.len(), 2);
}

#[test]
fn headline_summarizes_the_find() {
    let dir = TempDir::new();
    let result = ScanResult {
        discovered: vec![
            discovered("a", SourceKind::ollama(), "a"),
            discovered("b", SourceKind::file(), "b"),
        ],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::ollama(), SourceKind::file()],
        result,
    )]);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    let headline = summary.headline();
    assert!(headline.contains("2 models"), "{headline}");
    assert!(headline.contains("1 in Ollama"), "{headline}");
    assert!(headline.contains("1 loose file"), "{headline}");
}

#[test]
fn an_empty_scan_has_an_empty_headline() {
    let dir = TempDir::new();
    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::ollama()],
        ScanResult::default(),
    )]);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(summary.total_count, 0);
    assert_eq!(summary.headline(), "No models found on this Mac yet.");
}

#[test]
fn deduplicates_the_same_model_seen_by_two_scanners() {
    let dir = TempDir::new();
    let one = ScanResult {
        discovered: vec![discovered("dup", SourceKind::ollama(), "same")],
        ..Default::default()
    };
    let two = ScanResult {
        discovered: vec![discovered("dup", SourceKind::ollama(), "same")],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![
        scanner(vec![SourceKind::ollama()], one),
        scanner(vec![SourceKind::ollama()], two),
    ]);
    let mut registry = registry(&dir);
    service.discover(&mut registry).expect("discover");
    // Same source → same id → registered once.
    assert_eq!(registry.len(), 1);
}

const MIB: usize = 1 << 20;

#[test]
fn content_fingerprint_samples_head_and_tail_of_large_files() {
    let dir = TempDir::new();
    let make = |name: &str, middle: u8, tail: u8| {
        let path = dir.path().join(name);
        let mut bytes = vec![b'H'; MIB]; // head
        bytes.extend(vec![middle; MIB]); // middle (not sampled)
        bytes.extend(vec![tail; MIB]); // tail
        std::fs::write(&path, bytes).unwrap();
        content_fingerprint(&path).expect("fingerprint")
    };
    // Same head+tail, different middle → same fingerprint (middle isn't sampled).
    assert_eq!(make("a.bin", b'm', b'T'), make("b.bin", b'X', b'T'));
    // Different tail → different fingerprint.
    assert_ne!(make("a.bin", b'm', b'T'), make("c.bin", b'm', b'Z'));
}

#[test]
fn a_new_record_without_a_fingerprint_never_migrates() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let mut orphan = record("orphan", SourceKind::file(), "/old");
    orphan.state = ModelState::Missing;
    orphan.content_fingerprint = Some("deadbeef".to_owned());
    orphan.alias = Some("keep-me".to_owned());
    let orphan_id = orphan.id.clone();
    registry.register(orphan).unwrap();

    // A discovered model with NO weight path → its record has no fingerprint.
    let mut model = discovered("newcomer", SourceKind::file(), "/new");
    model.primary_weight_path = None;
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::file()], result)]);
    service.discover(&mut registry).expect("discover");

    // The orphan is untouched (still present, still owns its alias).
    let kept = registry.get(&orphan_id).expect("orphan retained");
    assert_eq!(kept.alias.as_deref(), Some("keep-me"));
}

#[test]
fn ambiguous_migration_candidates_are_not_claimed() {
    let dir = TempDir::new();
    let weight = dir.path().join("w.gguf");
    std::fs::write(&weight, b"weights").unwrap();
    let print = content_fingerprint(&weight).unwrap();

    let mut registry = registry(&dir);
    // TWO missing records with the same fingerprint + footprint.
    for tag in ["one", "two"] {
        let mut old = record(tag, SourceKind::file(), &format!("/old/{tag}"));
        old.state = ModelState::Missing;
        old.content_fingerprint = Some(print.clone());
        old.footprint_mb = Some(0);
        registry.register(old).unwrap();
    }

    let mut model = discovered("newcomer", SourceKind::file(), "/new");
    model.primary_weight_path = Some(weight.to_string_lossy().into_owned());
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::file()], result)]);
    service.discover(&mut registry).expect("discover");

    // Ambiguous (2 candidates) → no migration; both orphans are retained.
    assert_eq!(
        registry
            .list()
            .iter()
            .filter(|r| r.state == ModelState::Missing)
            .count(),
        2
    );
    // And the newcomer registered on its own.
    assert!(registry.list().iter().any(|r| r.name == "newcomer"));
}

#[test]
fn a_record_of_an_unscanned_kind_is_left_alone() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    // An LM Studio record with no weights, but only Ollama is scanned.
    let lms = record("lms", SourceKind::lm_studio(), "/nonexistent");
    let id = lms.id.clone();
    registry.register(lms).unwrap();

    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::ollama()],
        ScanResult::default(),
    )]);
    service.discover(&mut registry).expect("discover");
    // Its kind wasn't scanned, so it is not marked missing.
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Unresolved);
}

#[test]
fn an_unchanged_weight_reuses_the_stored_fingerprint_without_rehashing() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    // Existing record with a bogus fingerprint and a nonexistent weight path.
    let mut existing = record("m", SourceKind::ollama(), "same");
    existing.content_fingerprint = Some("BOGUS".to_owned());
    existing.primary_weight_path = Some("/nonexistent.gguf".to_owned());
    existing.footprint_mb = Some(0);
    let id = existing.id.clone();
    registry.register(existing).unwrap();

    // Re-discover the same source, weight path, and footprint.
    let mut model = discovered("m", SourceKind::ollama(), "same");
    model.primary_weight_path = Some("/nonexistent.gguf".to_owned());
    model.footprint_bytes = 5; // → 0 MiB, matching
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);
    service.discover(&mut registry).expect("discover");

    // Same path + footprint → the stored fingerprint is reused (not recomputed to
    // None from the missing file).
    assert_eq!(
        registry.get(&id).unwrap().content_fingerprint.as_deref(),
        Some("BOGUS")
    );
}
