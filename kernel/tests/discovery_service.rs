//! Tests for `DiscoveryService`: registry reconciliation (insert/update/missing),
//! the weights-present guard, moved-config migration, and the summary.

mod support;

use kernel::discovery::{
    DiscoveredModel, DiscoveryService, DiscoverySummary, KindStat, ScanResult, StoreScanner,
    content_fingerprint,
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
    assert_eq!(
        alpha.footprint_bytes,
        Some(5),
        "the exact bytes, not a truncated unit"
    );
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
fn an_empty_capability_hint_keeps_what_the_record_holds() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let existing = record("same", SourceKind::ollama(), "same");
    let id = existing.id.clone();
    registry.register(existing).unwrap();

    let mut model = discovered("same", SourceKind::ollama(), "same");
    model.capabilities_hint = Vec::new();
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);

    service.discover(&mut registry).expect("discover");
    assert_eq!(
        registry.get(&id).expect("still there").capabilities,
        vec![Capability::chat()]
    );
}

#[test]
fn a_modality_that_moved_takes_the_old_capabilities_with_it() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    // A reranker a shelf once read as an embedder, now hinted as the text model
    // it is, with nothing it can do until a runtime says so.
    let existing = ModelRecord::new(
        "rerank",
        Modality::embedding(),
        vec![Capability::embed()],
        ModelSource::new(SourceKind::huggingface_cache(), "rerank"),
    );
    let id = existing.id.clone();
    registry.register(existing).unwrap();

    let mut model = discovered("rerank", SourceKind::huggingface_cache(), "rerank");
    model.capabilities_hint = Vec::new();
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let service =
        DiscoveryService::new(vec![scanner(vec![SourceKind::huggingface_cache()], result)]);

    service.discover(&mut registry).expect("discover");
    let updated = registry.get(&id).expect("still there");
    assert_eq!(updated.modality, Modality::text());
    assert!(updated.capabilities.is_empty());
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
    old.footprint_bytes = Some(5);
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

    let model_a = loose("a", &a);
    let model_b = loose("b", &b);

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
    let group = &summary.duplicates[0];
    assert_eq!(group.removable.len(), 1);
    assert_eq!(group.reclaimable_bytes, 10);
    let (first, second) = (&group.kept.member, &group.removable[0].copy.member);
    for (member, name, path) in [(first, "a", &a), (second, "b", &b)] {
        assert_eq!(member.name, name);
        assert_eq!(member.kind, SourceKind::file());
        assert_eq!(member.path, path.to_string_lossy());
        assert!(registry.get(&member.id).is_some(), "{member:?}");
    }
}

/// A loose model named `name` whose file, and weights, are at `path`.
fn loose(name: &str, path: &std::path::Path) -> DiscoveredModel {
    let path = path.to_string_lossy();
    let mut model = discovered(name, SourceKind::file(), &path);
    model.primary_weight_path = Some(path.into_owned());
    model
}

/// Two loose models at `a.gguf` and `b.gguf` in `dir`, holding the same bytes.
fn twin_files(dir: &TempDir) -> (DiscoveredModel, DiscoveredModel) {
    let a = dir.path().join("a.gguf");
    let b = dir.path().join("b.gguf");
    std::fs::write(&a, b"same-bytes").unwrap();
    std::fs::write(&b, b"same-bytes").unwrap();
    (loose("a", &a), loose("b", &b))
}

#[test]
fn summary_names_a_duplicate_by_its_alias() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let mut aliased = record(
        "a",
        SourceKind::file(),
        &dir.path().join("a.gguf").to_string_lossy(),
    );
    aliased.alias = Some("favourite".to_owned());
    let id = aliased.id.clone();
    registry.register(aliased).unwrap();

    let (model_a, model_b) = twin_files(&dir);
    let result = ScanResult {
        discovered: vec![model_a, model_b],
        ..Default::default()
    };
    let service =
        DiscoveryService::with_threshold(vec![scanner(vec![SourceKind::file()], result)], 1);
    let summary = service.discover(&mut registry).expect("discover");
    let group = &summary.duplicates[0];
    let member = std::iter::once(&group.kept.member)
        .chain(group.removable.iter().map(|copy| &copy.copy.member))
        .find(|member| member.id == id)
        .expect("the aliased model is a member");
    assert_eq!(member.name, "favourite");
}

#[test]
fn summary_counts_a_model_two_scanners_report_once() {
    let dir = TempDir::new();
    let report = || ScanResult {
        discovered: vec![discovered("dup", SourceKind::ollama(), "same")],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![
        scanner(vec![SourceKind::ollama()], report()),
        scanner(vec![SourceKind::ollama()], report()),
    ]);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(summary.total_count, 1);
    assert_eq!(summary.per_kind[&SourceKind::ollama()].count, 1);
    assert_eq!(summary.per_kind[&SourceKind::ollama()].bytes, 5);
    assert_eq!(summary.total_bytes, 5);
}

#[test]
fn summary_lists_the_id_of_every_model_it_found() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let stale = record("stale", SourceKind::ollama(), "stale");
    registry.register(stale.clone()).unwrap();
    let found = discovered("found", SourceKind::ollama(), "found");
    let result = ScanResult {
        discovered: vec![found.clone(), found],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(vec![SourceKind::ollama()], result)]);
    let summary = service.discover(&mut registry).expect("discover");
    let ids: Vec<&String> = summary.found_ids.iter().collect();
    assert_eq!(ids.len(), 1);
    assert!(!summary.found_ids.contains(&stale.id));
    assert!(registry.get(ids[0]).is_some());
}

#[cfg(unix)]
#[test]
fn summary_counts_each_file_once_across_every_store() {
    let dir = TempDir::new();
    let blob = dir.path().join("blob");
    let config = dir.path().join("config");
    let linked = dir.path().join("linked.gguf");
    let own = dir.path().join("own.gguf");
    std::fs::write(&blob, vec![1u8; 700]).unwrap();
    std::fs::write(&config, vec![2u8; 30]).unwrap();
    std::fs::write(&own, vec![3u8; 200]).unwrap();
    std::fs::hard_link(&blob, &linked).unwrap();
    let path = |path: &std::path::Path| path.to_string_lossy().into_owned();

    let mut latest = discovered("qwen:latest", SourceKind::ollama(), "latest");
    latest.files = vec![path(&blob), path(&config)];
    let mut tagged = discovered("qwen:7b", SourceKind::ollama(), "7b");
    tagged.files = vec![path(&blob), path(&config)];
    let mut studio = discovered("Qwen-Q4", SourceKind::lm_studio(), "studio");
    studio.files = vec![path(&linked)];
    let mut loose = discovered("own", SourceKind::file(), "own");
    loose.files = vec![path(&own)];
    let unlisted = discovered("builtin", SourceKind::builtin(), "builtin");

    let result = ScanResult {
        discovered: vec![studio, loose, latest, tagged, unlisted],
        ..Default::default()
    };
    let service = DiscoveryService::new(vec![scanner(
        vec![
            SourceKind::ollama(),
            SourceKind::lm_studio(),
            SourceKind::file(),
            SourceKind::builtin(),
        ],
        result,
    )]);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(summary.per_kind[&SourceKind::ollama()], stat(2, 730));
    assert_eq!(
        summary.per_kind[&SourceKind::lm_studio()],
        stat(1, 0),
        "its file is the blob Ollama already counted"
    );
    assert_eq!(summary.per_kind[&SourceKind::file()], stat(1, 200));
    assert_eq!(
        summary.per_kind[&SourceKind::builtin()],
        stat(1, 5),
        "a model listing no files counts its footprint"
    );
    assert_eq!(summary.total_count, 5);
    assert_eq!(summary.total_bytes, 935);
}

#[test]
fn summary_compares_and_counts_every_shard_of_a_set() {
    let dir = TempDir::new();
    let set = |sub: &str, second: u8| {
        let folder = dir.path().join(sub);
        std::fs::create_dir_all(&folder).unwrap();
        let first = folder.join("m-00001-of-00002.gguf");
        let other = folder.join("m-00002-of-00002.gguf");
        std::fs::write(&first, [1u8; 60]).unwrap();
        std::fs::write(&other, vec![second; 40]).unwrap();
        loose(sub, &first)
    };
    let result = ScanResult {
        discovered: vec![set("one", 2), set("two", 3), set("three", 2)],
        ..Default::default()
    };
    let service =
        DiscoveryService::with_threshold(vec![scanner(vec![SourceKind::file()], result)], 1);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(the_only_group(&summary), ("one", vec![("three", 100)]));
    assert_eq!(summary.reclaimable_bytes, 100);
}

/// The name of the copy the summary's only duplicate group keeps, then the
/// name of each copy it offers with what removing that copy frees.
fn the_only_group(summary: &DiscoverySummary) -> (&str, Vec<(&str, i64)>) {
    assert_eq!(summary.duplicates.len(), 1, "{:?}", summary.duplicates);
    let group = &summary.duplicates[0];
    let removable = group
        .removable
        .iter()
        .map(|copy| (copy.copy.member.name.as_str(), copy.reclaimable_bytes))
        .collect();
    (group.kept.member.name.as_str(), removable)
}

#[test]
fn summary_offers_a_hugging_face_repo_only_when_another_keeps_its_every_blob() {
    let dir = TempDir::new();
    let repo = |name: &str, blobs: &[(&str, u8, usize)]| {
        let root = dir.path().join(format!("models--org--{name}"));
        std::fs::create_dir_all(root.join("blobs")).unwrap();
        for (blob, fill, size) in blobs {
            std::fs::write(root.join("blobs").join(blob), vec![*fill; *size]).unwrap();
        }
        let mut model = discovered(
            name,
            SourceKind::huggingface_cache(),
            &root.to_string_lossy(),
        );
        model.primary_weight_path = Some(root.join("blobs/q8").to_string_lossy().into_owned());
        model
    };
    let result = ScanResult {
        discovered: vec![
            repo("MQ-A", &[("q8", 8, 200), ("q4", 4, 100)]),
            repo("MQ-B", &[("q8", 8, 200)]),
            repo("MQ-C", &[("q8", 8, 200), ("q4", 4, 100)]),
        ],
        ..Default::default()
    };
    let service = DiscoveryService::with_threshold(
        vec![scanner(vec![SourceKind::huggingface_cache()], result)],
        1,
    );
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(
        the_only_group(&summary),
        ("MQ-A", vec![("MQ-B", 200), ("MQ-C", 300)]),
        "MQ-B holds only the Q8, so it can go for MQ-A, and MQ-A never for it"
    );
}

#[test]
fn summary_compares_every_file_of_a_folder_bundle() {
    let dir = TempDir::new();
    let bundle = |name: &str, second: u8, finder: &[u8], extra: bool| {
        let root = dir.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.json"), b"{}").unwrap();
        let first = root.join("model-00001-of-00002.safetensors");
        std::fs::write(&first, [1u8; 300]).unwrap();
        std::fs::write(
            root.join("model-00002-of-00002.safetensors"),
            vec![second; 200],
        )
        .unwrap();
        std::fs::write(root.join(".DS_Store"), finder).unwrap();
        if extra {
            std::fs::create_dir_all(root.join("onnx")).unwrap();
            std::fs::write(root.join("onnx/model.onnx"), [9u8; 50]).unwrap();
        }
        let mut model = discovered(name, SourceKind::folder(), &root.to_string_lossy());
        model.primary_weight_path = Some(first.to_string_lossy().into_owned());
        model
    };
    let finder = |rest: &[u8]| [b"\x00\x00\x00\x01Bud1".as_slice(), rest].concat();
    let one = finder(b"one");
    let another = finder(b"another");
    let result = ScanResult {
        discovered: vec![
            bundle("fb1", 2, &one, false),
            bundle("fb2", 3, &one, false),
            bundle("fb3", 2, &another, false),
            bundle("fb4", 2, &one, true),
            bundle("fb5", 2, b"no Finder wrote this", false),
        ],
        ..Default::default()
    };
    let service =
        DiscoveryService::with_threshold(vec![scanner(vec![SourceKind::folder()], result)], 1);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(
        the_only_group(&summary),
        (
            "fb4",
            vec![
                ("fb1", 502 + one.len() as i64),
                ("fb3", 502 + another.len() as i64)
            ]
        ),
        "fb2 differs in its second shard, fb4 holds one more file than fb1 and fb3, \
         each Finder .DS_Store is freed with its bundle, and fb5's is no Finder's"
    );
}

#[test]
fn summary_offers_an_lm_studio_clone_of_an_ollama_model_and_keeps_the_ollama_model() {
    let dir = TempDir::new();
    let root = dir.path().join("ollama");
    std::fs::create_dir_all(root.join("blobs")).unwrap();
    std::fs::write(
        root.join("blobs/sha256-1111111111111111111111111111111111111111111111111111111111111111"),
        vec![7u8; 400],
    )
    .unwrap();
    std::fs::write(
        root.join("blobs/sha256-2222222222222222222222222222222222222222222222222222222222222222"),
        b"{{ .Prompt }}",
    )
    .unwrap();
    let tags = root.join("manifests/registry.ollama.ai/library/qwen");
    std::fs::create_dir_all(&tags).unwrap();
    let manifest = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","layers":[
        {"mediaType":"application/vnd.ollama.image.model","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","size":400},
        {"mediaType":"application/vnd.ollama.image.template","digest":"sha256:2222222222222222222222222222222222222222222222222222222222222222","size":13}
    ]}"#;
    let mut ollama = Vec::new();
    for tag in ["7b", "latest"] {
        std::fs::write(tags.join(tag), manifest).unwrap();
        let mut model = discovered(
            &format!("qwen:{tag}"),
            SourceKind::ollama(),
            &tags.join(tag).to_string_lossy(),
        );
        model.primary_weight_path = Some(
            root.join(
                "blobs/sha256-1111111111111111111111111111111111111111111111111111111111111111",
            )
            .to_string_lossy()
            .into_owned(),
        );
        ollama.push(model);
    }
    let clone = dir.path().join("Qwen-Q4.gguf");
    std::fs::write(&clone, vec![7u8; 400]).unwrap();
    let mut studio = discovered("Qwen-Q4", SourceKind::lm_studio(), &clone.to_string_lossy());
    studio.primary_weight_path = Some(clone.to_string_lossy().into_owned());
    let mut found = ollama;
    found.push(studio);
    let result = ScanResult {
        discovered: found,
        ..Default::default()
    };
    let service = DiscoveryService::with_threshold(
        vec![scanner(
            vec![SourceKind::ollama(), SourceKind::lm_studio()],
            result,
        )],
        1,
    );
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert_eq!(
        the_only_group(&summary),
        ("qwen:7b", vec![("Qwen-Q4", 400)]),
        "the template blob has no counterpart in LM Studio"
    );
    let aliases: Vec<&str> = summary.duplicates[0]
        .kept
        .aliases
        .iter()
        .map(|alias| alias.name.as_str())
        .collect();
    assert_eq!(aliases, vec!["qwen:latest"]);
}

#[test]
fn summary_leaves_a_downloading_model_out_of_duplicates() {
    let dir = TempDir::new();
    let (model_a, mut model_b) = twin_files(&dir);
    model_b.downloading = true;
    let result = ScanResult {
        discovered: vec![model_a, model_b],
        ..Default::default()
    };
    let service =
        DiscoveryService::with_threshold(vec![scanner(vec![SourceKind::file()], result)], 1);
    let mut registry = registry(&dir);
    let summary = service.discover(&mut registry).expect("discover");
    assert!(summary.duplicates.is_empty(), "{:?}", summary.duplicates);
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
    assert_eq!(
        headline,
        "Found 2 models on this Mac (1 in Ollama, 1 loose file). Total: 10 B."
    );
    assert!(!headline.contains('\u{2014}'), "{headline}");
}

fn stat(count: usize, bytes: i64) -> KindStat {
    KindStat { count, bytes }
}

#[test]
fn stores_follow_the_headline_order_and_skip_empty_kinds() {
    let mut summary = DiscoverySummary::default();
    summary.per_kind.insert(SourceKind::file(), stat(1, 10));
    summary.per_kind.insert(SourceKind::ollama(), stat(2, 20));
    summary
        .per_kind
        .insert(SourceKind::lm_studio(), stat(3, 30));
    summary
        .per_kind
        .insert(SourceKind::huggingface_cache(), stat(0, 0));
    summary.per_kind.insert(SourceKind::folder(), stat(1, 5));
    let kinds: Vec<SourceKind> = summary.stores().into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(
        kinds,
        vec![
            SourceKind::ollama(),
            SourceKind::lm_studio(),
            SourceKind::file(),
            SourceKind::folder(),
        ]
    );
    assert_eq!(summary.stores()[1].1, stat(3, 30));
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
        old.footprint_bytes = Some(5);
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
    existing.footprint_bytes = Some(5);
    let id = existing.id.clone();
    registry.register(existing).unwrap();

    // Re-discover the same source, weight path, and footprint.
    let mut model = discovered("m", SourceKind::ollama(), "same");
    model.primary_weight_path = Some("/nonexistent.gguf".to_owned());
    model.footprint_bytes = 5; // the same five bytes, so the print still matches
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

#[test]
fn a_scan_copies_the_serving_figure_and_clears_a_stale_one() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let mut model = discovered("multi", SourceKind::huggingface_cache(), "/hub/multi");
    model.footprint_bytes = 300;
    model.serving_bytes = Some(200);
    let first = ScanResult {
        discovered: vec![model.clone()],
        ..Default::default()
    };
    let kinds = vec![SourceKind::huggingface_cache()];
    DiscoveryService::new(vec![scanner(kinds.clone(), first)])
        .discover(&mut registry)
        .expect("discover");
    let id = kernel::records::stable_id(&model.source);
    assert_eq!(registry.get(&id).unwrap().serving_bytes, Some(200));

    model.serving_bytes = None;
    let second = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    DiscoveryService::new(vec![scanner(kinds, second)])
        .discover(&mut registry)
        .expect("discover");
    let record = registry.get(&id).unwrap();
    assert_eq!(record.serving_bytes, None);
    assert_eq!(record.footprint_bytes, Some(300));
}

#[test]
fn the_summary_totals_stay_on_disk() {
    let dir = TempDir::new();
    let mut registry = registry(&dir);
    let mut model = discovered("multi", SourceKind::huggingface_cache(), "/hub/multi");
    model.footprint_bytes = 300;
    model.serving_bytes = Some(200);
    let result = ScanResult {
        discovered: vec![model],
        ..Default::default()
    };
    let summary =
        DiscoveryService::new(vec![scanner(vec![SourceKind::huggingface_cache()], result)])
            .discover(&mut registry)
            .expect("discover");
    assert_eq!(summary.total_bytes, 300);
    assert_eq!(
        summary.per_kind[&SourceKind::huggingface_cache()].bytes,
        300
    );
}

#[cfg(unix)]
#[test]
fn a_loose_file_that_became_a_link_to_a_directory_is_marked_missing() {
    let dir = TempDir::new();
    let target = dir.path().join("elsewhere");
    std::fs::create_dir_all(&target).unwrap();
    let weight = dir.path().join("fake.gguf");
    std::os::unix::fs::symlink(&target, &weight).unwrap();
    let weight = weight.to_string_lossy().into_owned();
    let mut registry = registry(&dir);
    // What an older build registered for it: the file is its own source.
    let mut fake = record("fake", SourceKind::file(), &weight);
    fake.primary_weight_path = Some(weight);
    let id = fake.id.clone();
    registry.register(fake).unwrap();

    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::file()],
        ScanResult::default(),
    )]);
    service.discover(&mut registry).expect("discover");
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Missing);
}

#[test]
fn a_repo_directory_still_stands_for_weights_its_primary_moved_within() {
    let dir = TempDir::new();
    let repo = dir.path().join("models--org--repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut registry = registry(&dir);
    let mut hub = record(
        "repo",
        SourceKind::huggingface_cache(),
        &repo.to_string_lossy(),
    );
    hub.primary_weight_path = Some(repo.join("gone.safetensors").to_string_lossy().into_owned());
    let id = hub.id.clone();
    registry.register(hub).unwrap();

    let service = DiscoveryService::new(vec![scanner(
        vec![SourceKind::huggingface_cache()],
        ScanResult::default(),
    )]);
    service.discover(&mut registry).expect("discover");
    assert_eq!(registry.get(&id).unwrap().state, ModelState::Unresolved);
}

/// Discover `found` with a threshold of one byte.
fn summarize(
    dir: &TempDir,
    kinds: Vec<SourceKind>,
    found: Vec<DiscoveredModel>,
) -> DiscoverySummary {
    let result = ScanResult {
        discovered: found,
        ..Default::default()
    };
    let service = DiscoveryService::with_threshold(vec![scanner(kinds, result)], 1);
    let mut registry = registry(dir);
    service.discover(&mut registry).expect("discover")
}

/// Every copy offered, by name, with what removing it alone frees.
fn offered(summary: &DiscoverySummary) -> Vec<(&str, i64)> {
    let mut offered: Vec<(&str, i64)> = summary
        .duplicates
        .iter()
        .flat_map(|group| &group.removable)
        .map(|copy| (copy.copy.member.name.as_str(), copy.reclaimable_bytes))
        .collect();
    offered.sort();
    offered
}

fn write(path: &std::path::Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A loose model at `path` holding `bytes` that cannot go itself: a hard link
/// outside the shelf keeps its file.
#[cfg(unix)]
fn pinned_loose(dir: &TempDir, name: &str, bytes: &[u8]) -> DiscoveredModel {
    let path = dir.path().join("Models").join(format!("{name}.gguf"));
    write(&path, bytes);
    let outside = dir.path().join("outside").join(format!("{name}.gguf"));
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::hard_link(&path, &outside).unwrap();
    loose(name, &path)
}

/// A commit hash, as the hub writes one under a repo's `refs/`.
const COMMIT: &[u8] = b"0123456789abcdef0123456789abcdef01234567";

/// A Hugging Face repo under `hub` holding `bytes` as its one blob, linked
/// from a snapshot, with a ref naming the snapshot's commit.
#[cfg(unix)]
fn hub_repo(
    hub: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> (std::path::PathBuf, DiscoveredModel) {
    let root = hub.join(format!("models--org--{name}"));
    write(&root.join("blobs/w"), bytes);
    write(&root.join("refs/main"), COMMIT);
    std::fs::create_dir_all(root.join("snapshots/r1")).unwrap();
    std::os::unix::fs::symlink("../../blobs/w", root.join("snapshots/r1/w.gguf")).unwrap();
    let mut model = discovered(
        name,
        SourceKind::huggingface_cache(),
        &root.to_string_lossy(),
    );
    model.primary_weight_path = Some(root.join("blobs/w").to_string_lossy().into_owned());
    (root, model)
}

#[cfg(unix)]
#[test]
fn a_repo_is_offered_only_when_another_keeps_every_file_its_removal_deletes() {
    let dir = TempDir::new();
    let weights = vec![8u8; 400];
    let hub = dir.path().join("hub");
    let (_, plain) = hub_repo(&hub, "Plain", &weights);
    let (marked, mut with_marker) = hub_repo(&hub, "Marked", &weights);
    write(&marked.join(".no_exist/r1/adapter_config.json"), b"");
    with_marker.name = "Marked".to_owned();
    let (real, snapshot_file) = hub_repo(&hub, "SnapshotFile", &weights);
    write(&real.join("snapshots/r1/w.Q2_K.gguf"), &[5u8; 300]);
    let (root, root_file) = hub_repo(&hub, "RootFile", &weights);
    write(&root.join("notes.bin"), &[6u8; 20]);
    let (old, older_snapshot) = hub_repo(&hub, "OlderSnapshot", &weights);
    write(&old.join("snapshots/r0/w.gguf"), &[7u8; 400]);

    let summary = summarize(
        &dir,
        vec![SourceKind::file(), SourceKind::huggingface_cache()],
        vec![
            pinned_loose(&dir, "keeper", &weights),
            plain,
            with_marker,
            snapshot_file,
            root_file,
            older_snapshot,
        ],
    );
    let freed = offered(&summary)
        .into_iter()
        .filter(|(name, _)| !["Marked", "Plain"].contains(name))
        .collect::<Vec<_>>();
    assert!(
        freed.is_empty(),
        "a repo with a file the keeper lacks is never offered: {freed:?}"
    );
    let plain_freed: Vec<i64> = offered(&summary)
        .into_iter()
        .filter(|(name, _)| ["Marked", "Plain"].contains(name))
        .map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(
        plain_freed,
        vec![440, 440],
        "the blob and the 40-byte ref; the empty marker and the snapshot link need no counterpart"
    );
}

#[cfg(unix)]
#[test]
fn a_repo_reached_through_a_symlink_is_never_offered() {
    let dir = TempDir::new();
    let weights = vec![8u8; 400];
    let hub = dir.path().join("hub");
    let (outside, _) = hub_repo(&dir.path().join("ext"), "Linked", &weights);
    std::fs::create_dir_all(&hub).unwrap();
    let linked_root = hub.join("models--org--Linked");
    std::os::unix::fs::symlink(&outside, &linked_root).unwrap();
    let mut linked = discovered(
        "Linked",
        SourceKind::huggingface_cache(),
        &linked_root.to_string_lossy(),
    );
    linked.primary_weight_path = Some(linked_root.join("blobs/w").to_string_lossy().into_owned());

    let (blobs_elsewhere, _) = hub_repo(&dir.path().join("ext2"), "LinkedBlobs", &weights);
    let repo = hub.join("models--org--LinkedBlobs");
    std::fs::create_dir_all(repo.join("snapshots/r1")).unwrap();
    std::os::unix::fs::symlink(blobs_elsewhere.join("blobs"), repo.join("blobs")).unwrap();
    std::os::unix::fs::symlink("../../blobs/w", repo.join("snapshots/r1/w.gguf")).unwrap();
    let mut linked_blobs = discovered(
        "LinkedBlobs",
        SourceKind::huggingface_cache(),
        &repo.to_string_lossy(),
    );
    linked_blobs.primary_weight_path = Some(repo.join("blobs/w").to_string_lossy().into_owned());

    let summary = summarize(
        &dir,
        vec![SourceKind::file(), SourceKind::huggingface_cache()],
        vec![pinned_loose(&dir, "keeper", &weights), linked, linked_blobs],
    );
    assert!(offered(&summary).is_empty(), "{:?}", summary.duplicates);
    assert_eq!(summary.reclaimable_bytes, 0);
}

#[cfg(unix)]
#[test]
fn a_file_two_spellings_reach_is_one_directory_entry_against_its_hard_links() {
    let dir = TempDir::new();
    let models = dir.path().join("Models");
    let z = models.join("z.gguf");
    write(&z, &[3u8; 400]);
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    std::fs::hard_link(&z, dir.path().join("outside/z.gguf")).unwrap();
    let alias = dir.path().join("Alias");
    std::os::unix::fs::symlink(&models, &alias).unwrap();
    let clone = dir.path().join("Keep/c.gguf");
    write(&clone, &[3u8; 400]);

    let summary = summarize(
        &dir,
        vec![SourceKind::file()],
        vec![
            loose("z", &z),
            loose("z", &alias.join("z.gguf")),
            loose("c", &clone),
        ],
    );
    assert_eq!(
        offered(&summary),
        vec![("c", 400)],
        "removing both spellings of z deletes one entry, and the outside link keeps the file"
    );
}

/// A folder bundle `name` holding `files`, its first file the primary.
fn bundle(dir: &TempDir, name: &str, files: &[(&str, &[u8])]) -> DiscoveredModel {
    let root = dir.path().join(name);
    for (file, bytes) in files {
        write(&root.join(file), bytes);
    }
    let mut model = discovered(name, SourceKind::folder(), &root.to_string_lossy());
    model.primary_weight_path = Some(root.join(files[0].0).to_string_lossy().into_owned());
    model
}

#[test]
fn a_same_size_file_elsewhere_never_changes_the_keeper_or_the_figure() {
    let dir = TempDir::new();
    let base: [(&str, &[u8]); 3] = [
        ("model-00001-of-00002.safetensors", &[1u8; 300]),
        ("model-00002-of-00002.safetensors", &[2u8; 200]),
        ("config.json", b"{}"),
    ];
    let mut bigger = base.to_vec();
    bigger.push(("onnx/model.onnx", &[9u8; 50]));
    let mut bystander = base.to_vec();
    bystander.push(("extra/adapter.safetensors", &[8u8; 50]));
    let summary = summarize(
        &dir,
        vec![SourceKind::folder()],
        vec![
            bundle(&dir, "fb1", &base),
            bundle(&dir, "fb3", &base),
            bundle(&dir, "fb4", &bigger),
            bundle(&dir, "zsub", &bystander),
        ],
    );
    assert_eq!(
        the_only_group(&summary),
        ("fb4", vec![("fb1", 502), ("fb3", 502)])
    );
    assert_eq!(summary.reclaimable_bytes, 1004);
}

#[test]
fn removing_copies_that_share_a_hard_link_frees_it_once_in_the_total() {
    let dir = TempDir::new();
    let shard1: &[u8] = &[1u8; 300];
    let shard2: &[u8] = &[2u8; 200];
    let keeper = bundle(
        &dir,
        "k",
        &[
            ("s-1.safetensors", shard1),
            ("s-2.safetensors", shard2),
            ("README", b"own"),
        ],
    );
    let x1 = bundle(
        &dir,
        "x1",
        &[("s-1.safetensors", shard1), ("s-2.safetensors", shard2)],
    );
    let x2_root = dir.path().join("x2");
    std::fs::create_dir_all(&x2_root).unwrap();
    std::fs::hard_link(
        dir.path().join("x1/s-1.safetensors"),
        x2_root.join("s-1.safetensors"),
    )
    .unwrap();
    write(&x2_root.join("s-2.safetensors"), shard2);
    let mut x2 = discovered("x2", SourceKind::folder(), &x2_root.to_string_lossy());
    x2.primary_weight_path = Some(
        x2_root
            .join("s-1.safetensors")
            .to_string_lossy()
            .into_owned(),
    );

    let summary = summarize(&dir, vec![SourceKind::folder()], vec![keeper, x1, x2]);
    assert_eq!(
        the_only_group(&summary),
        ("k", vec![("x1", 200), ("x2", 200)]),
        "each alone frees only its own second shard"
    );
    assert_eq!(summary.duplicates[0].reclaimable_bytes, 700);
    assert_eq!(
        summary.reclaimable_bytes, 700,
        "together they free the shared shard too"
    );
}

/// An Ollama model `tag` under `root` whose one layer is `weights`, with a
/// config blob and no template or parameters.
fn bare_ollama(root: &std::path::Path, tag: &str, weights: &[u8]) -> DiscoveredModel {
    write(
        &root.join("blobs/sha256-1111111111111111111111111111111111111111111111111111111111111111"),
        weights,
    );
    write(
        &root.join("blobs/sha256-3333333333333333333333333333333333333333333333333333333333333333"),
        b"{\"model_format\":\"gguf\"}",
    );
    let manifest = root
        .join("manifests/registry.ollama.ai/library/bare")
        .join(tag);
    write(
        &manifest,
        br#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:3333333333333333333333333333333333333333333333333333333333333333","size":23},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","size":400}]}"#,
    );
    let mut model = discovered(
        &format!("bare:{tag}"),
        SourceKind::ollama(),
        &manifest.to_string_lossy(),
    );
    model.primary_weight_path = Some(
        root.join("blobs/sha256-1111111111111111111111111111111111111111111111111111111111111111")
            .to_string_lossy()
            .into_owned(),
    );
    model
}

#[cfg(unix)]
#[test]
fn a_bare_ollama_model_can_go_for_a_copy_that_cannot_and_frees_its_manifest_and_config() {
    let dir = TempDir::new();
    let weights = vec![4u8; 400];
    let root = dir.path().join("ollama");
    let ollama = bare_ollama(&root, "latest", &weights);
    let mut studio = pinned_loose(&dir, "s", &weights);
    studio.source.kind = SourceKind::lm_studio();
    let manifest = std::fs::metadata(&ollama.source.path).unwrap().len() as i64;
    let summary = summarize(
        &dir,
        vec![SourceKind::ollama(), SourceKind::lm_studio()],
        vec![ollama, studio],
    );
    assert_eq!(
        the_only_group(&summary),
        ("s", vec![("bare:latest", 400 + 23 + manifest)])
    );
}

#[cfg(unix)]
#[test]
fn a_copy_with_an_ollama_name_is_kept_before_a_loose_one() {
    let dir = TempDir::new();
    let weights = vec![4u8; 400];
    let root = dir.path().join("ollama");
    let ollama = bare_ollama(&root, "latest", &weights);
    let linked = dir.path().join("Models/aaa-u.gguf");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::fs::hard_link(
        root.join("blobs/sha256-1111111111111111111111111111111111111111111111111111111111111111"),
        &linked,
    )
    .unwrap();
    let studio_path = dir.path().join("studio/u.gguf");
    write(&studio_path, &weights);
    let mut studio = loose("u", &studio_path);
    studio.source.kind = SourceKind::lm_studio();
    let summary = summarize(
        &dir,
        vec![
            SourceKind::ollama(),
            SourceKind::lm_studio(),
            SourceKind::file(),
        ],
        vec![ollama, loose("aaa-u", &linked), studio],
    );
    assert_eq!(the_only_group(&summary), ("aaa-u", vec![("u", 400)]));
    assert_eq!(summary.duplicates[0].kept.aliases[0].name, "bare:latest");
}

#[cfg(unix)]
#[test]
fn a_copy_holding_real_files_beside_a_link_is_not_called_a_link() {
    let dir = TempDir::new();
    let store = dir.path().join("store/w.safetensors");
    write(&store, &[1u8; 300]);
    let b1 = dir.path().join("B1");
    std::fs::create_dir_all(&b1).unwrap();
    std::os::unix::fs::symlink(&store, b1.join("model.safetensors")).unwrap();
    write(&b1.join("second.safetensors"), &[2u8; 280]);
    let mut linked = discovered("B1", SourceKind::folder(), &b1.to_string_lossy());
    linked.primary_weight_path = Some(b1.join("model.safetensors").to_string_lossy().into_owned());
    let summary = summarize(
        &dir,
        vec![SourceKind::folder()],
        vec![
            linked,
            bundle(
                &dir,
                "B2",
                &[
                    ("model.safetensors", &[1u8; 300]),
                    ("second.safetensors", &[2u8; 280]),
                ],
            ),
        ],
    );
    let group = &summary.duplicates[0];
    for copy in std::iter::once(&group.kept).chain(group.removable.iter().map(|copy| &copy.copy)) {
        assert_eq!(copy.link_target, None, "{copy:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_ref_is_bookkeeping_only_when_it_holds_a_commit_hash() {
    let dir = TempDir::new();
    let weights = vec![8u8; 400];
    let hub = dir.path().join("hub");
    let (notes, with_notes) = hub_repo(&hub, "Notes", &weights);
    write(&notes.join("refs/notes.bin"), &[1u8; 2000]);
    let (backup, with_backup) = hub_repo(&hub, "Backup", &weights);
    write(&dir.path().join("ext/backup.bin"), &[2u8; 500]);
    std::fs::hard_link(
        dir.path().join("ext/backup.bin"),
        backup.join("refs/backup.bin"),
    )
    .unwrap();
    let (stash, with_stash) = hub_repo(&hub, "Stash", &[8u8; 400]);
    write(
        &stash.join("refs/stash"),
        b"not a hash, but small enough to pass for one by its size alone",
    );
    let (pull, with_pull) = hub_repo(&hub, "Pull", &weights);
    let mut hash_and_newline = COMMIT.to_vec();
    hash_and_newline.push(b'\n');
    write(&pull.join("refs/pr/1"), &hash_and_newline);

    let summary = summarize(
        &dir,
        vec![SourceKind::file(), SourceKind::huggingface_cache()],
        vec![
            pinned_loose(&dir, "keeper", &weights),
            with_notes,
            with_backup,
            with_stash,
            with_pull,
        ],
    );
    assert_eq!(
        offered(&summary),
        vec![("Pull", 400 + 40 + 41)],
        "a file under refs/ that is no commit hash is content the keeper lacks"
    );
}

#[cfg(unix)]
#[test]
fn finder_metadata_is_freed_and_counted_and_a_lookalike_is_content() {
    let dir = TempDir::new();
    let weights = vec![8u8; 400];
    let finder = |rest: &[u8]| [b"\x00\x00\x00\x01Bud1".as_slice(), rest].concat();
    let hub = dir.path().join("hub");
    let (shown, finder_repo) = hub_repo(&hub, "Shown", &weights);
    write(&shown.join(".DS_Store"), &finder(&[0u8; 92]));
    write(&shown.join("snapshots/r1/.DS_Store"), &finder(&[1u8; 92]));
    write(
        &shown.join("snapshots/r1/._w.gguf"),
        &[b"\x00\x05\x16\x07".as_slice(), &[0u8; 46]].concat(),
    );
    let (faked, faked_repo) = hub_repo(&hub, "Faked", &weights);
    write(&faked.join(".DS_Store"), &[3u8; 3000]);
    let (large, large_repo) = hub_repo(&hub, "Large", &weights);
    write(&large.join(".DS_Store"), &finder(&vec![4u8; 2 << 20]));

    let summary = summarize(
        &dir,
        vec![SourceKind::file(), SourceKind::huggingface_cache()],
        vec![
            pinned_loose(&dir, "keeper", &weights),
            finder_repo,
            faked_repo,
            large_repo,
        ],
    );
    assert_eq!(
        offered(&summary),
        vec![("Shown", 400 + 40 + 100 + 100 + 50)],
        "Finder's files go with the repo and count; one no Finder wrote is content"
    );
}

/// An Ollama model `tag` of `model` under `root`: a manifest naming each of
/// `layers` (a media type, the hex digit its digest repeats, its bytes) and
/// a config blob holding `config`, as the daemon lays them out.
fn ollama_tag(
    root: &std::path::Path,
    model: &str,
    tag: &str,
    layers: &[(&str, char, &[u8])],
    config: &[u8],
) -> DiscoveredModel {
    let digest = |digit: char| digit.to_string().repeat(64);
    let mut listed = Vec::new();
    for (media, digit, bytes) in layers {
        write(
            &root.join(format!("blobs/sha256-{}", digest(*digit))),
            bytes,
        );
        listed.push(serde_json::json!({
            "mediaType": media,
            "digest": format!("sha256:{}", digest(*digit)),
            "size": bytes.len(),
        }));
    }
    let config_digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(config));
    write(&root.join(format!("blobs/sha256-{config_digest}")), config);
    let manifest = root
        .join("manifests/registry.ollama.ai/library")
        .join(model)
        .join(tag);
    write(
        &manifest,
        serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "config": {
                "mediaType": "application/vnd.docker.container.image.v1+json",
                "digest": format!("sha256:{config_digest}"),
                "size": config.len(),
            },
            "layers": listed,
        })
        .to_string()
        .as_bytes(),
    );
    let mut found = discovered(
        &format!("{model}:{tag}"),
        SourceKind::ollama(),
        &manifest.to_string_lossy(),
    );
    found.primary_weight_path = layers
        .iter()
        .find(|(media, ..)| media.ends_with(".model"))
        .map(|(_, digit, _)| {
            root.join(format!("blobs/sha256-{}", digest(*digit)))
                .to_string_lossy()
                .into_owned()
        });
    found
}

const MODEL: &str = "application/vnd.ollama.image.model";
const LICENSE: &str = "application/vnd.ollama.image.license";
const CONFIG_JSON: &[u8] = br#"{"model_format":"gguf","model_family":"llama","model_families":["llama"],"model_type":"1B","file_type":"Q8_0","architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[]}}"#;

#[cfg(unix)]
#[test]
fn an_ollama_manifest_or_config_is_bookkeeping_only_as_the_daemon_writes_it() {
    let dir = TempDir::new();
    let weights = vec![4u8; 400];
    let root = dir.path().join("ollama");
    let plain = ollama_tag(
        &root,
        "plain",
        "latest",
        &[(MODEL, 'a', &weights)],
        CONFIG_JSON,
    );
    let noted = ollama_tag(
        &root,
        "noted",
        "latest",
        &[(MODEL, 'a', &weights)],
        CONFIG_JSON,
    );
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&noted.source.path).unwrap()).unwrap();
    manifest["notes"] = serde_json::json!("a field the daemon never writes");
    write(
        std::path::Path::new(&noted.source.path),
        manifest.to_string().as_bytes(),
    );
    let configured = ollama_tag(
        &root,
        "configured",
        "latest",
        &[(MODEL, 'a', &weights)],
        br#"{"model_format":"gguf","diary":"three megabytes of someone's own words"}"#,
    );

    let summary = summarize(
        &dir,
        vec![SourceKind::ollama(), SourceKind::file()],
        vec![
            pinned_loose(&dir, "keeper", &weights),
            plain,
            noted,
            configured,
        ],
    );
    let names: Vec<&str> = offered(&summary)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        names,
        vec!["plain:latest"],
        "a manifest or config with a field the daemon never writes is content"
    );
}

#[cfg(unix)]
#[test]
fn a_blob_a_model_with_no_weights_lists_is_not_counted_as_freed() {
    let dir = TempDir::new();
    let weights = vec![4u8; 400];
    let license = vec![b'L'; 300];
    let keeper = bundle(
        &dir,
        "keeper",
        &[("model.safetensors", &weights), ("LICENSE", &license)],
    );
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    for file in ["model.safetensors", "LICENSE"] {
        std::fs::hard_link(
            dir.path().join("keeper").join(file),
            dir.path().join("outside").join(file),
        )
        .unwrap();
    }
    let root = dir.path().join("ollama");
    let layers: [(&str, char, &[u8]); 2] = [(MODEL, 'a', &weights), (LICENSE, 'b', &license)];
    let local = ollama_tag(&root, "local", "latest", &layers, CONFIG_JSON);
    let cloud = ollama_tag(
        &root,
        "cloud",
        "latest",
        &[(LICENSE, 'b', &license)],
        br#"{"model_format":"gguf","remote_host":"https://ollama.com:443","remote_model":"m"}"#,
    );
    let written = |model: &DiscoveredModel| {
        let manifest = std::fs::metadata(&model.source.path).unwrap().len() as i64;
        manifest + CONFIG_JSON.len() as i64
    };
    let alone = written(&local);
    let summary = summarize(
        &dir,
        vec![SourceKind::ollama(), SourceKind::folder()],
        vec![keeper, local, cloud],
    );
    assert_eq!(
        offered(&summary),
        vec![("local:latest", 400 + alone)],
        "the daemon keeps the license blob while the cloud tag lists it"
    );
}

#[cfg(unix)]
#[test]
fn the_target_a_shelf_model_reaches_through_a_symlink_is_never_offered() {
    let weights = vec![8u8; 400];
    let linked_in = |dir: &TempDir, link: bool| {
        let (target, beta) = hub_repo(&dir.path().join("hub2"), "Beta", &weights);
        let mut found = vec![pinned_loose(dir, "keeper", &weights), beta];
        if link {
            let hub = dir.path().join("hub");
            std::fs::create_dir_all(&hub).unwrap();
            let spelled = hub.join("models--org--Beta");
            std::os::unix::fs::symlink(&target, &spelled).unwrap();
            let mut through = discovered(
                "Beta",
                SourceKind::huggingface_cache(),
                &spelled.to_string_lossy(),
            );
            through.primary_weight_path =
                Some(spelled.join("blobs/w").to_string_lossy().into_owned());
            found.push(through);
        }
        summarize(
            dir,
            vec![SourceKind::file(), SourceKind::huggingface_cache()],
            found,
        )
    };
    let alone = TempDir::new();
    assert_eq!(offered(&linked_in(&alone, false)), vec![("Beta", 440)]);
    let linked = TempDir::new();
    assert!(
        offered(&linked_in(&linked, true)).is_empty(),
        "removing hub2's Beta would leave the linked Beta with nothing"
    );
}

#[cfg(unix)]
#[test]
fn a_bundle_another_reaches_through_a_chain_of_links_is_never_offered() {
    let dir = TempDir::new();
    let weights = vec![5u8; 400];
    let keeper = bundle(&dir, "keeper", &[("model.safetensors", &weights)]);
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    std::fs::hard_link(
        dir.path().join("keeper/model.safetensors"),
        dir.path().join("outside/model.safetensors"),
    )
    .unwrap();
    let target = bundle(&dir, "target", &[("model.safetensors", &weights)]);
    let spare = bundle(&dir, "spare", &[("model.safetensors", &weights)]);
    let user = bundle(&dir, "user", &[("model.safetensors", &[6u8; 400])]);
    std::os::unix::fs::symlink(dir.path().join("target"), dir.path().join("hop")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("hop"), dir.path().join("user/sub")).unwrap();

    let summary = summarize(
        &dir,
        vec![SourceKind::folder()],
        vec![keeper, target, spare, user],
    );
    assert_eq!(
        offered(&summary),
        vec![("spare", 400)],
        "user reaches target through hop, so target stays"
    );
    let group = &summary.duplicates[0];
    assert_eq!(group.kept.member.name, "keeper");
}

/// Whether names under `dir` match whatever their case, as on a default
/// macOS volume, where a link can be typed in a case the disk never wrote.
#[cfg(unix)]
fn case_insensitive(dir: &TempDir) -> bool {
    let probe = dir.path().join("case-probe");
    write(&probe, b"");
    dir.path().join("CASE-PROBE").exists()
}

#[cfg(unix)]
#[test]
fn a_link_typed_in_another_case_still_keeps_the_repo_it_reaches() {
    let dir = TempDir::new();
    if !case_insensitive(&dir) {
        return;
    }
    let weights = vec![8u8; 400];
    let hub = dir.path().join("hub");
    let mut found = Vec::new();
    for name in ["A", "B"] {
        let (root, repo) = hub_repo(&hub, name, &weights);
        write(&root.join("snapshots/r1/config.json"), b"{}");
        found.push(repo);
    }
    let link = dir.path().join("Models/l.gguf");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(dir.path().join("HUB/models--org--B/blobs/w"), &link).unwrap();
    found.push(loose("l", &link));

    let summary = summarize(
        &dir,
        vec![SourceKind::file(), SourceKind::huggingface_cache()],
        found,
    );
    assert_eq!(
        offered(&summary),
        vec![("A", 400 + 40 + 2)],
        "l reaches B's blob through HUB, so B stays and A goes"
    );
}

#[cfg(unix)]
#[test]
fn a_bundle_a_link_typed_in_another_case_reaches_is_never_offered() {
    let dir = TempDir::new();
    if !case_insensitive(&dir) {
        return;
    }
    let weights = vec![5u8; 400];
    let keeper = bundle(&dir, "keeper", &[("model.safetensors", &weights)]);
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    std::fs::hard_link(
        dir.path().join("keeper/model.safetensors"),
        dir.path().join("outside/model.safetensors"),
    )
    .unwrap();
    let target = bundle(&dir, "target", &[("model.safetensors", &weights)]);
    let mut user = bundle(&dir, "user", &[("config.json", b"{\"user\":1}")]);
    let linked = dir.path().join("user/model.safetensors");
    std::os::unix::fs::symlink(dir.path().join("TARGET/model.safetensors"), &linked).unwrap();
    user.primary_weight_path = Some(linked.to_string_lossy().into_owned());

    let summary = summarize(&dir, vec![SourceKind::folder()], vec![keeper, target, user]);
    assert!(
        offered(&summary).is_empty(),
        "user reaches target's weights through TARGET, so target stays"
    );
}

#[cfg(unix)]
#[test]
fn a_bundle_holding_a_link_another_model_passes_in_another_case_is_never_offered() {
    let dir = TempDir::new();
    if !case_insensitive(&dir) {
        return;
    }
    let weights = vec![6u8; 400];
    let keeper = bundle(&dir, "keeper", &[("model.safetensors", &weights)]);
    let target = bundle(&dir, "target", &[("model.safetensors", &weights)]);
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    write(&elsewhere.join("model.safetensors"), b"other weights");
    std::os::unix::fs::symlink(&elsewhere, dir.path().join("target/hop")).unwrap();
    let mut user = bundle(&dir, "user", &[("config.json", b"{\"user\":1}")]);
    let through = dir.path().join("target/HOP/model.safetensors");
    user.primary_weight_path = Some(through.to_string_lossy().into_owned());

    let summary = summarize(&dir, vec![SourceKind::folder()], vec![keeper, target, user]);
    assert!(
        !offered(&summary).iter().any(|(name, _)| *name == "target"),
        "user passes target's hop link as HOP, so removing target would break it"
    );
}

/// A folder bundle holding `weights` that cannot go: a hard link outside the
/// shelf keeps its file.
#[cfg(unix)]
fn pinned_bundle(dir: &TempDir, name: &str, weights: &[u8]) -> DiscoveredModel {
    let model = bundle(dir, name, &[("model.safetensors", weights)]);
    std::fs::create_dir_all(dir.path().join("outside")).unwrap();
    std::fs::hard_link(
        dir.path().join(name).join("model.safetensors"),
        dir.path().join("outside").join(name),
    )
    .unwrap();
    model
}

#[cfg(unix)]
#[test]
fn only_finder_and_appledouble_files_in_their_shape_go_as_metadata() {
    let dir = TempDir::new();
    let weights = vec![5u8; 400];
    let finder = |rest: &[u8]| [b"\x00\x00\x00\x01Bud1".as_slice(), rest].concat();
    let fork = |len: usize| [b"\x00\x05\x16\x07".as_slice(), &vec![7u8; len]].concat();
    let keeper = pinned_bundle(&dir, "keeper", &weights);
    let large = bundle(
        &dir,
        "large",
        &[
            ("model.safetensors", &weights),
            (".DS_Store", &finder(&vec![1u8; 900_000])),
        ],
    );
    let orphan = bundle(
        &dir,
        "orphan",
        &[
            ("model.safetensors", &weights),
            ("._orphan-notes", &fork(800_000)),
        ],
    );
    let described = bundle(
        &dir,
        "described",
        &[
            ("model.safetensors", &weights),
            ("._model.safetensors", &fork(500)),
            (".DS_Store", &finder(&[2u8; 92])),
        ],
    );

    let summary = summarize(
        &dir,
        vec![SourceKind::folder()],
        vec![keeper, large, orphan, described],
    );
    assert_eq!(
        offered(&summary),
        vec![("described", 400 + 504 + 100)],
        "a 900 KB .DS_Store and a ._ file describing nothing removed are content"
    );
}

/// A scanner over `watched`, reporting `result` as found there.
struct Watching {
    watched: Vec<std::path::PathBuf>,
    result: ScanResult,
}

impl StoreScanner for Watching {
    fn kinds(&self) -> Vec<SourceKind> {
        vec![SourceKind::file(), SourceKind::huggingface_cache()]
    }
    fn scan(&self) -> ScanResult {
        self.result.clone()
    }
    fn watched_directories(&self) -> Vec<std::path::PathBuf> {
        self.watched.clone()
    }
}

#[cfg(unix)]
#[test]
fn a_repo_whose_removal_deletes_a_watched_folder_is_never_offered() {
    let dir = TempDir::new();
    let weights = vec![8u8; 400];
    let hub = dir.path().join("hub");
    let (inside, inside_repo) = hub_repo(&hub, "Inside", &weights);
    let (chained, chained_repo) = hub_repo(&hub, "Chained", &weights);
    let (_, free_repo) = hub_repo(&hub, "Free", &weights);
    std::os::unix::fs::symlink(chained.join("snapshots/r1"), dir.path().join("d1")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("d1"), dir.path().join("W")).unwrap();
    let watched = vec![inside.join("snapshots/r1"), dir.path().join("W")];

    let service = DiscoveryService::with_threshold(
        vec![Box::new(Watching {
            watched,
            result: ScanResult {
                discovered: vec![
                    pinned_loose(&dir, "keeper", &weights),
                    inside_repo,
                    chained_repo,
                    free_repo,
                ],
                ..Default::default()
            },
        })],
        1,
    );
    let summary = service.discover(&mut registry(&dir)).expect("discover");
    assert_eq!(
        offered(&summary),
        vec![("Free", 440)],
        "removing Inside or Chained would delete a watched folder"
    );
}
