//! Tests for the runtime-bid auction: bid collection and ranking, applying the
//! winner (with alternatives) to a record, the pin/missing/downloading guards,
//! merging identified facts, and the `explain` dry-run.

mod support;

use std::sync::Arc;

use kernel::records::{
    Capability, Modality, ModelRecord, ModelSource, Resolution, RunTier, RuntimeId, SourceKind,
};
use kernel::registry::Registry;
use kernel::resolution::{IdentificationCache, IdentifiedModel, RuntimeBid};
use runtime::adapters::{ChunkStream, OllamaAdapter, RuntimeAdapter, RuntimeError};
use runtime::resolution::ResolutionEngine;
use support::TempDir;

/// An adapter that offers a fixed bid regardless of the model.
struct FakeAdapter {
    id: RuntimeId,
    bid: Option<RuntimeBid>,
    wires_tools: bool,
}

impl FakeAdapter {
    fn arced(id: RuntimeId, bid: Option<RuntimeBid>) -> Arc<dyn RuntimeAdapter> {
        Arc::new(Self {
            id,
            bid,
            wires_tools: false,
        })
    }

    /// A fake that, like the real ollama/llama.cpp/openai adapters, forwards
    /// tools to its backend.
    fn tool_wiring(id: RuntimeId, bid: Option<RuntimeBid>) -> Arc<dyn RuntimeAdapter> {
        Arc::new(Self {
            id,
            bid,
            wires_tools: true,
        })
    }
}

impl RuntimeAdapter for FakeAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        self.wires_tools
    }

    fn can_serve(&self, _record: &ModelRecord, capability: &Capability) -> bool {
        *capability == Capability::chat()
    }

    fn bid(&self, _record: &ModelRecord, _identified: &IdentifiedModel) -> Option<RuntimeBid> {
        self.bid.clone()
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        _payload: kernel::records::JsonValue,
    ) -> ChunkStream {
        ChunkStream::failed(RuntimeError::Failed("unused".into()))
    }
}

fn registry() -> (TempDir, Registry) {
    let dir = TempDir::new();
    let registry = Registry::open(dir.path()).expect("registry");
    (dir, registry)
}

fn file_record() -> ModelRecord {
    ModelRecord::new(
        "m",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/nonexistent/model"),
    )
}

/// An engine with a single ollama adapter that always bids native/20.
fn ollama_engine() -> ResolutionEngine {
    ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )])
}

#[test]
fn the_lowest_preference_bid_wins_and_the_rest_become_alternatives() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![
        FakeAdapter::arced(
            RuntimeId::mlx_lm(),
            Some(RuntimeBid::new(RunTier::Managed, 40)),
        ),
        FakeAdapter::arced(
            RuntimeId::ollama(),
            Some(RuntimeBid::new(RunTier::Native, 20)),
        ),
    ]);
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(updated.runtime.tier, RunTier::Native);
    assert_eq!(updated.runtime.resolved, Resolution::Auto);
    assert_eq!(updated.state, kernel::records::ModelState::Ready);
    assert_eq!(updated.runtime.alternatives, vec![RuntimeId::mlx_lm()]);
}

#[test]
fn the_winners_declared_alternatives_are_appended_without_duplicates() {
    let (_dir, mut reg) = registry();
    // The winner declares mlx_lm (already a runner-up), itself, and a fresh id.
    let winner_bid = RuntimeBid::with_alternatives(
        RunTier::Native,
        20,
        vec![
            RuntimeId::mlx_lm(),
            RuntimeId::ollama(),
            RuntimeId::whisper_cpp(),
        ],
    );
    let engine = ResolutionEngine::new(vec![
        FakeAdapter::arced(RuntimeId::ollama(), Some(winner_bid)),
        FakeAdapter::arced(
            RuntimeId::mlx_lm(),
            Some(RuntimeBid::new(RunTier::Managed, 40)),
        ),
    ]);
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    // Runner-up first, then the winner's fresh declared alternative; no self, no dup.
    assert_eq!(
        updated.runtime.alternatives,
        vec![RuntimeId::mlx_lm(), RuntimeId::whisper_cpp()]
    );
}

#[test]
fn no_bids_leaves_the_record_unresolved() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(RuntimeId::ollama(), None)]);
    let mut record = file_record();
    // Start it resolved so we can see it get reset.
    record.state = kernel::records::ModelState::Ready;
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, None);
    assert_eq!(updated.runtime.tier, RunTier::RecipeNeeded);
    assert_eq!(updated.state, kernel::records::ModelState::Unresolved);
}

#[test]
fn a_missing_record_is_not_resolved() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let mut record = file_record();
    record.state = kernel::records::ModelState::Missing;
    reg.register(record.clone()).unwrap();

    assert!(engine.resolve(&record, &mut reg).unwrap().is_none());
}

#[test]
fn a_user_pinned_record_on_a_live_adapter_is_left_alone() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let mut record = file_record();
    record.runtime.resolved = Resolution::User;
    record.runtime.id = Some(RuntimeId::ollama());
    reg.register(record.clone()).unwrap();

    assert!(engine.resolve(&record, &mut reg).unwrap().is_none());
}

#[test]
fn a_user_pin_to_a_dead_adapter_is_re_resolved() {
    let (_dir, mut reg) = registry();
    // Pinned to whisper_cpp, but only ollama is live → the pin is stale.
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let mut record = file_record();
    record.runtime.resolved = Resolution::User;
    record.runtime.id = Some(RuntimeId::whisper_cpp());
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(updated.runtime.resolved, Resolution::Auto);
}

#[test]
fn a_downloading_record_is_reset_to_unresolved() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let mut record = file_record();
    record.downloading = true;
    record.state = kernel::records::ModelState::Ready;
    record.runtime.id = Some(RuntimeId::ollama());
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, None);
    assert_eq!(updated.state, kernel::records::ModelState::Unresolved);
}

#[test]
fn resolution_merges_identified_facts_onto_the_record() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    // An ollama record identifies as chat/complete text, overriding the bare
    // capability set the record was registered with.
    let record = ModelRecord::new(
        "llama",
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::ollama(), "/nonexistent/manifest"),
    );
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert!(updated.capabilities.contains(&Capability::chat()));
    assert_eq!(updated.modality, Modality::text());
}

#[test]
fn confirmed_at_survives_an_unchanged_winner_but_clears_on_a_switch() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let mut record = file_record();
    record.runtime.id = Some(RuntimeId::ollama());
    record.runtime.resolved = Resolution::Auto;
    record.runtime.confirmed_at = Some(1234);
    reg.register(record.clone()).unwrap();

    // Same winner → the confirmation timestamp is kept.
    let updated = engine.resolve(&record, &mut reg).unwrap();
    let confirmed = updated.map_or(record.runtime.confirmed_at, |r| r.runtime.confirmed_at);
    assert_eq!(confirmed, Some(1234));

    // A different live winner → the timestamp is cleared.
    let engine2 = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::mlx_lm(),
        Some(RuntimeBid::new(RunTier::Managed, 40)),
    )]);
    let updated2 = engine2
        .resolve(&record, &mut reg)
        .unwrap()
        .expect("changed");
    assert_eq!(updated2.runtime.id, Some(RuntimeId::mlx_lm()));
    assert_eq!(updated2.runtime.confirmed_at, None);
}

#[test]
fn resolve_all_filters_by_source_kind() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let file = file_record();
    let ollama = ModelRecord::new(
        "o",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), "/nonexistent/manifest"),
    );
    reg.register(file.clone()).unwrap();
    reg.register(ollama.clone()).unwrap();

    let mut kinds = std::collections::HashSet::new();
    kinds.insert(SourceKind::ollama());
    let changed = engine.resolve_all(&mut reg, Some(&kinds)).unwrap();
    // Only the ollama record was in scope.
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].id, ollama.id);
}

#[test]
fn explain_ranks_bids_winner_first_without_mutating() {
    let engine = ResolutionEngine::new(vec![
        FakeAdapter::arced(
            RuntimeId::mlx_lm(),
            Some(RuntimeBid::new(RunTier::Managed, 40)),
        ),
        FakeAdapter::arced(
            RuntimeId::ollama(),
            Some(RuntimeBid::new(RunTier::Native, 20)),
        ),
        FakeAdapter::arced(RuntimeId::whisper_cpp(), None),
    ]);
    let record = file_record();
    let explanation = engine.explain(&record);

    assert_eq!(explanation.bids.len(), 2);
    assert_eq!(explanation.winner(), Some(&RuntimeId::ollama()));
    assert_eq!(explanation.bids[0].preference, 20);
    assert_eq!(explanation.bids[1].adapter_id, RuntimeId::mlx_lm());
}

#[test]
fn the_real_ollama_adapter_bids_on_an_ollama_model() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![Arc::new(OllamaAdapter::new())]);
    let record = ModelRecord::new(
        "llama3",
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::ollama(), "/nonexistent/manifest"),
    );
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(updated.runtime.tier, RunTier::Native);
}

#[test]
fn re_resolving_an_already_resolved_record_reports_no_change() {
    let (_dir, mut reg) = registry();
    let engine = ollama_engine();
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let resolved = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    // Feeding the resolved record back yields the same plan → no diff → None.
    assert!(engine.resolve(&resolved, &mut reg).unwrap().is_none());
}

#[test]
fn applied_re_guards_against_registry_state_the_argument_does_not_reflect() {
    let (_dir, mut reg) = registry();
    let engine = ollama_engine();
    // The stored record is user-pinned to the live ollama adapter, so it must be
    // left alone — even though the argument we resolve with looks unpinned.
    let mut stored = file_record();
    stored.runtime.resolved = Resolution::User;
    stored.runtime.id = Some(RuntimeId::ollama());
    reg.register(stored.clone()).unwrap();

    let unpinned = file_record(); // same id, but resolved == Auto
    assert_eq!(unpinned.id, stored.id);
    assert!(engine.resolve(&unpinned, &mut reg).unwrap().is_none());
}

#[test]
fn identification_without_params_backfills_from_the_profile_registry() {
    let (_dir, mut reg) = registry();
    // A plain file record identifies as Unknown with no param schema, so the
    // builtin text-generation profile fills the schema in.
    let engine = ollama_engine();
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert!(updated.params.iter().any(|spec| spec.key == "temperature"));
}

#[test]
fn an_empty_profile_registry_leaves_the_schema_empty() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::with_profiles(
        vec![FakeAdapter::arced(
            RuntimeId::ollama(),
            Some(RuntimeBid::new(RunTier::Native, 20)),
        )],
        kernel::profiles::ProfileRegistry::new(Vec::new()),
    );
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    // Winner is set, but with no profiles the text-generation schema
    // (temperature/top_p/…) is never backfilled.
    assert_eq!(updated.runtime.id, Some(RuntimeId::ollama()));
    assert!(!updated.params.iter().any(|spec| spec.key == "temperature"));
}

#[test]
fn equal_preference_ties_break_on_the_runtime_id() {
    let engine = ResolutionEngine::new(vec![
        FakeAdapter::arced(
            RuntimeId::ollama(),
            Some(RuntimeBid::new(RunTier::Native, 20)),
        ),
        FakeAdapter::arced(
            RuntimeId::mlx_lm(),
            Some(RuntimeBid::new(RunTier::Managed, 20)),
        ),
    ]);
    let explanation = engine.explain(&file_record());
    // Both bid 20; the lexicographically-smaller id wins the tie.
    let expected = std::cmp::min(RuntimeId::ollama(), RuntimeId::mlx_lm());
    assert_eq!(explanation.winner(), Some(&expected));
}

#[test]
fn a_sole_bidder_still_records_its_declared_alternatives() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::with_alternatives(
            RunTier::Native,
            20,
            vec![RuntimeId::whisper_cpp()],
        )),
    )]);
    let record = file_record();
    reg.register(record.clone()).unwrap();

    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert_eq!(updated.runtime.id, Some(RuntimeId::ollama()));
    assert_eq!(updated.runtime.alternatives, vec![RuntimeId::whisper_cpp()]);
}

#[test]
fn with_cache_consults_the_identification_cache_across_resolution_passes() {
    let (_dir, mut reg) = registry();
    // A real on-disk GGUF file so the cache can key on a freshness signature.
    let file_dir = TempDir::new();
    let gguf = file_dir.path().join("model.gguf");
    std::fs::write(&gguf, b"GGUF").unwrap();

    let cache = Arc::new(IdentificationCache::new());
    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )])
    .with_cache(Arc::clone(&cache));

    let record = ModelRecord::new(
        "m",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), gguf.to_str().unwrap()),
    );
    reg.register(record).unwrap();

    // First pass populates the cache; a second pass over the unchanged shelf hits.
    engine.resolve_all(&mut reg, None).unwrap();
    assert_eq!(cache.hit_count(), 0);
    engine.resolve_all(&mut reg, None).unwrap();
    assert!(cache.hit_count() >= 1);
}

#[test]
fn explain_all_covers_every_record() {
    let (_dir, mut reg) = registry();
    let engine = ollama_engine();
    reg.register(file_record()).unwrap();
    reg.register(ModelRecord::new(
        "o",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::ollama(), "/nonexistent/manifest"),
    ))
    .unwrap();

    let explanations = engine.explain_all(&reg);
    assert_eq!(explanations.len(), 2);
    // The ollama-kind record draws the ollama bid; the loose file draws none.
    let ollama_bid = explanations
        .iter()
        .find(|e| e.record.source.kind == SourceKind::ollama())
        .expect("ollama explanation");
    assert_eq!(ollama_bid.winner(), Some(&RuntimeId::ollama()));
}

#[test]
fn resolve_folds_tools_only_when_the_winning_adapter_wires_them() {
    let (_dir, mut reg) = registry();
    let engine = ResolutionEngine::new(vec![FakeAdapter::tool_wiring(
        RuntimeId::ollama(),
        Some(RuntimeBid::new(RunTier::Native, 20)),
    )]);
    let record = file_record();
    reg.register(record.clone()).unwrap();
    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert!(updated.capabilities.contains(&Capability::tools()));

    // The same model behind an adapter that renders only the message list (a
    // Python sidecar) must not offer tools.
    let (_dir, mut reg) = registry();
    let engine = ollama_engine();
    let record = file_record();
    reg.register(record.clone()).unwrap();
    let updated = engine.resolve(&record, &mut reg).unwrap().expect("changed");
    assert!(!updated.capabilities.contains(&Capability::tools()));
}

#[test]
fn refold_migrates_a_stale_record_without_rerunning_the_auction() {
    let (_dir, mut reg) = registry();
    // A record resolved before the `tools` capability existed: runtime already
    // assigned, capabilities lack tools.
    let mut record = file_record();
    record.runtime.id = Some(RuntimeId::ollama());
    record.state = kernel::records::ModelState::Ready;
    reg.register(record).unwrap();

    let engine = ResolutionEngine::new(vec![FakeAdapter::tool_wiring(RuntimeId::ollama(), None)]);
    let changed = engine.refold_tool_capability(&mut reg).unwrap();
    assert_eq!(changed.len(), 1);
    assert!(changed[0].capabilities.contains(&Capability::tools()));
    // The auction was not re-run: the runtime assignment is untouched.
    assert_eq!(changed[0].runtime.id, Some(RuntimeId::ollama()));

    // Idempotent: a second pass changes nothing.
    assert!(engine.refold_tool_capability(&mut reg).unwrap().is_empty());
}

#[test]
fn refold_withholds_tools_from_a_record_on_a_non_wiring_runtime() {
    let (_dir, mut reg) = registry();
    let mut record = file_record();
    record.runtime.id = Some(RuntimeId::mlx_lm());
    record.capabilities.push(Capability::tools());
    reg.register(record).unwrap();

    let engine = ResolutionEngine::new(vec![FakeAdapter::arced(RuntimeId::mlx_lm(), None)]);
    let changed = engine.refold_tool_capability(&mut reg).unwrap();
    assert_eq!(changed.len(), 1);
    assert!(!changed[0].capabilities.contains(&Capability::tools()));
}
