//! Tests for GGUF encoders as boot wires the real adapter set: an encoder that
//! only embeds resolves to llama.cpp and is ready, a chat GGUF resolves exactly
//! as it did, each dispatches only its own capability, a GGUF that pools
//! nothing or ranks draws no bid, one with a decision head is served as a
//! judge and never as an embedder, and a shelf written
//! before encoders were served has its stranded embedder resolved on first
//! sight.

mod support;

use kernel::records::{
    Capability, Modality, ModelRecord, ModelSource, ModelState, Resolution, RunTier, RuntimeId,
    SourceKind,
};
use kernel::registry::Registry;
use runtime::boot::{HedosDirs, build_kernel};
use runtime::facade::{Kernel, KernelError};
use runtime::settings::SettingsStore;
use support::{TempDir, gguf, kv_string, kv_u32};

struct Fixture {
    root: TempDir,
    dirs: HedosDirs,
    settings: SettingsStore,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new();
        let dirs = HedosDirs {
            data: root.join("data"),
        };
        std::fs::create_dir_all(dirs.sub("registry")).unwrap();
        let settings = SettingsStore::new(root.join("hedos.toml"));
        Self {
            root,
            dirs,
            settings,
        }
    }

    /// Write `header` as `name` and register it the way a scan of a loose file
    /// does, before anything resolved it. Returns the record's id.
    fn register(&self, name: &str, header: &[Vec<u8>]) -> String {
        self.register_as(name, header, |_| {})
    }

    /// The same, with `shape` applied to the record first, for a row an older
    /// shelf wrote.
    fn register_as(
        &self,
        name: &str,
        header: &[Vec<u8>],
        shape: impl FnOnce(&mut ModelRecord),
    ) -> String {
        let path = self.root.join(name);
        std::fs::write(&path, gguf(header)).unwrap();
        let mut record = ModelRecord::new(
            name,
            Modality::unknown(),
            Vec::new(),
            ModelSource::new(SourceKind::file(), &path.to_string_lossy()),
        );
        shape(&mut record);
        let id = record.id.clone();
        self.registry().register(record).unwrap();
        id
    }

    fn registry(&self) -> Registry {
        Registry::open(&self.dirs.sub("registry")).unwrap()
    }

    /// Every record on disk, in the registry's order.
    fn records(&self) -> Vec<ModelRecord> {
        self.registry().list().into_iter().cloned().collect()
    }

    fn kernel(&self) -> Kernel {
        build_kernel(&self.dirs, &self.settings.load()).expect("build kernel")
    }

    async fn resolved(&self) -> Kernel {
        let kernel = self.kernel();
        kernel.resolve().await.expect("resolve");
        kernel
    }
}

fn nomic_embedder() -> Vec<Vec<u8>> {
    vec![
        kv_string("general.architecture", "nomic-bert"),
        kv_u32("nomic-bert.context_length", 2048),
        kv_u32("nomic-bert.pooling_type", 1),
    ]
}

fn llama_chat() -> Vec<Vec<u8>> {
    vec![
        kv_string("general.architecture", "llama"),
        kv_u32("llama.context_length", 4096),
        kv_string("tokenizer.chat_template", "{{ messages }}"),
    ]
}

fn on_shelf(shelf: &[ModelRecord], id: &str) -> ModelRecord {
    shelf
        .iter()
        .find(|record| record.id == id)
        .expect("the model is on the shelf")
        .clone()
}

#[tokio::test]
async fn a_gguf_embedder_resolves_to_llama_cpp_and_is_ready() {
    let fixture = Fixture::new();
    let id = fixture.register("nomic-embed.gguf", &nomic_embedder());

    let kernel = fixture.resolved().await;
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()));
    assert_eq!(record.runtime.resolved, Resolution::Auto);
    assert_eq!(record.runtime.tier, RunTier::Native);
    assert!(record.runtime.alternatives.is_empty());
    assert_eq!(record.state, ModelState::Ready);
    assert_eq!(record.modality, Modality::embedding());
    assert_eq!(record.capabilities, vec![Capability::embed()]);
}

#[tokio::test]
async fn a_chat_gguf_resolves_exactly_as_before() {
    let fixture = Fixture::new();
    let id = fixture.register("llama.gguf", &llama_chat());

    let kernel = fixture.resolved().await;
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()));
    assert!(record.runtime.alternatives.is_empty());
    assert_eq!(record.state, ModelState::Ready);
    for capability in [
        Capability::chat(),
        Capability::complete(),
        Capability::tools(),
    ] {
        assert!(record.capabilities.contains(&capability), "{capability:?}");
    }
    assert!(!record.capabilities.contains(&Capability::embed()));
}

#[tokio::test]
async fn each_gguf_dispatches_only_its_own_capability() {
    let fixture = Fixture::new();
    let embedder = fixture.register("nomic-embed.gguf", &nomic_embedder());
    let chat = fixture.register("llama.gguf", &llama_chat());
    let kernel = fixture.resolved().await;

    assert!(
        kernel
            .honored_params(&embedder, Capability::embed())
            .await
            .is_ok()
    );
    assert!(matches!(
        kernel.honored_params(&embedder, Capability::chat()).await,
        Err(KernelError::CapabilityUnsupported { .. })
    ));
    assert!(
        kernel
            .honored_params(&chat, Capability::chat())
            .await
            .is_ok()
    );
    assert!(matches!(
        kernel.honored_params(&chat, Capability::embed()).await,
        Err(KernelError::CapabilityUnsupported { .. })
    ));
}

#[tokio::test]
async fn a_ranking_bert_gguf_draws_no_bid() {
    let fixture = Fixture::new();
    let id = fixture.register(
        "bge-reranker.gguf",
        &[
            kv_string("general.architecture", "bert"),
            kv_u32("bert.pooling_type", 4),
        ],
    );

    let kernel = fixture.resolved().await;
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.state, ModelState::Unresolved);
    assert_eq!(record.runtime.id, None);
    assert!(record.capabilities.is_empty());
}

#[tokio::test]
async fn a_gguf_with_a_decision_head_is_served_as_a_judge_and_never_as_an_embedder() {
    let fixture = Fixture::new();
    let id = fixture.register(
        "laya.gguf",
        &[
            kv_string("general.architecture", "modern-bert"),
            kv_string("modern-bert.decision.type", "laya"),
            kv_u32("modern-bert.classifier.pooling_type", 1),
            kv_u32("modern-bert.pooling_type", 1),
        ],
    );

    let kernel = fixture.resolved().await;
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.state, ModelState::Ready);
    assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()));
    assert_eq!(record.capabilities, vec![Capability::judge()]);
    assert!(
        kernel
            .honored_params(&id, Capability::judge())
            .await
            .is_ok()
    );
    for capability in [Capability::embed(), Capability::chat()] {
        assert!(
            matches!(
                kernel.honored_params(&id, capability.clone()).await,
                Err(KernelError::CapabilityUnsupported { .. })
            ),
            "{capability:?}"
        );
    }
}

#[tokio::test]
async fn a_stranded_gguf_embedder_is_resolved_on_first_sight_of_the_shelf() {
    let fixture = Fixture::new();
    let id = fixture.register_as("nomic-embed.gguf", &nomic_embedder(), |record| {
        record.modality = Modality::embedding();
        record.capabilities = vec![Capability::embed()];
        record.state = ModelState::Unresolved;
    });

    // No resolve and no discover: what `hedos serve` does before its first request.
    let kernel = fixture.kernel();
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()));
    assert_eq!(record.state, ModelState::Ready);
    assert_eq!(record.capabilities, vec![Capability::embed()]);
}

#[tokio::test]
async fn an_unresolved_gguf_that_claims_nothing_is_left_alone() {
    let fixture = Fixture::new();
    let id = fixture.register_as(
        "mmproj-model-f16.gguf",
        &[kv_string("general.architecture", "clip")],
        |record| {
            record.modality = Modality::vision();
            record.state = ModelState::Unresolved;
        },
    );
    let before = fixture.records();

    let kernel = fixture.kernel();
    let record = on_shelf(&kernel.shelf().await, &id);
    assert_eq!(record.state, ModelState::Unresolved);
    assert_eq!(record.runtime.id, None);
    assert_eq!(fixture.records(), before);
}

#[tokio::test]
async fn a_stranded_embedder_whose_file_is_gone_or_unreadable_is_left_as_it_was() {
    let fixture = Fixture::new();
    let stranded = |record: &mut ModelRecord| {
        record.modality = Modality::embedding();
        record.capabilities = vec![Capability::embed()];
        record.state = ModelState::Unresolved;
    };
    let gone = fixture.register_as("deleted-embedder.gguf", &nomic_embedder(), stranded);
    std::fs::remove_file(fixture.root.join("deleted-embedder.gguf")).unwrap();
    let unreadable = fixture.register_as("garbled-embedder.gguf", &nomic_embedder(), stranded);
    std::fs::write(fixture.root.join("garbled-embedder.gguf"), b"GGUF\x03").unwrap();
    let before = fixture.records();

    let kernel = fixture.kernel();
    let shelf = kernel.shelf().await;
    for id in [&gone, &unreadable] {
        let record = on_shelf(&shelf, id);
        assert_eq!(record.state, ModelState::Unresolved);
        assert_eq!(record.runtime.id, None);
        assert_eq!(record.modality, Modality::embedding());
        assert_eq!(record.capabilities, vec![Capability::embed()]);
    }
    assert_eq!(fixture.records(), before);
    // Rows it could not read keep the shelf unmigrated, so a file written
    // whole by the next process is rescued then, without a rescan.
    assert_eq!(fixture.registry().migration_level(), 0);
    std::fs::write(
        fixture.root.join("garbled-embedder.gguf"),
        gguf(&nomic_embedder()),
    )
    .unwrap();
    std::fs::write(
        fixture.root.join("deleted-embedder.gguf"),
        gguf(&nomic_embedder()),
    )
    .unwrap();
    let shelf = fixture.kernel().shelf().await;
    for id in [&gone, &unreadable] {
        let record = on_shelf(&shelf, id);
        assert_eq!(record.state, ModelState::Ready);
        assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()));
    }
    assert_eq!(fixture.registry().migration_level(), 2);
}

#[tokio::test]
async fn a_stranded_decoder_embedder_is_resolved_and_a_stranded_reranker_claims_nothing() {
    let fixture = Fixture::new();
    let stranded = |record: &mut ModelRecord| {
        record.modality = Modality::embedding();
        record.capabilities = vec![Capability::embed()];
        record.state = ModelState::Unresolved;
    };
    let qwen = fixture.register_as(
        "qwen3-embedding.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.context_length", 32768),
            kv_u32("qwen3.pooling_type", 3),
            kv_string("tokenizer.chat_template", "{{ messages }}"),
        ],
        stranded,
    );
    let reranker = fixture.register_as(
        "bge-reranker.gguf",
        &[
            kv_string("general.architecture", "bert"),
            kv_u32("bert.pooling_type", 4),
        ],
        stranded,
    );

    let kernel = fixture.kernel();
    let shelf = kernel.shelf().await;
    let qwen = on_shelf(&shelf, &qwen);
    assert_eq!(qwen.runtime.id, Some(RuntimeId::llama_cpp()));
    assert_eq!(qwen.state, ModelState::Ready);
    assert_eq!(qwen.capabilities, vec![Capability::embed()]);
    let reranker = on_shelf(&shelf, &reranker);
    assert_eq!(reranker.runtime.id, None);
    assert!(reranker.capabilities.is_empty());
}

#[tokio::test]
async fn a_decoder_gguf_that_embeds_resolves_as_an_embedder_and_one_that_ranks_is_withheld() {
    let fixture = Fixture::new();
    let embedder = fixture.register(
        "qwen3-embedding.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.context_length", 32768),
            kv_u32("qwen3.pooling_type", 3),
            kv_string("tokenizer.chat_template", "{{ messages }}"),
        ],
    );
    let reranker = fixture.register(
        "qwen3-reranker.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.pooling_type", 4),
            kv_string("tokenizer.chat_template", "{{ messages }}"),
        ],
    );

    let kernel = fixture.resolved().await;
    let shelf = kernel.shelf().await;
    let embedder = on_shelf(&shelf, &embedder);
    assert_eq!(embedder.state, ModelState::Ready);
    assert_eq!(embedder.modality, Modality::embedding());
    assert_eq!(embedder.capabilities, vec![Capability::embed()]);
    assert!(matches!(
        kernel
            .honored_params(&embedder.id, Capability::chat())
            .await,
        Err(KernelError::CapabilityUnsupported { .. })
    ));
    let reranker = on_shelf(&shelf, &reranker);
    assert_eq!(reranker.state, ModelState::Unresolved);
    assert_eq!(reranker.runtime.id, None);
    assert!(reranker.capabilities.is_empty());
}

#[tokio::test]
async fn a_decoder_embedder_an_older_shelf_served_as_chat_is_resolved_on_first_sight() {
    let fixture = Fixture::new();
    let served_as_chat = |record: &mut ModelRecord| {
        record.modality = Modality::text();
        record.capabilities = vec![
            Capability::chat(),
            Capability::complete(),
            Capability::tools(),
        ];
        record.runtime.id = Some(RuntimeId::llama_cpp());
        record.runtime.resolved = Resolution::Auto;
        record.runtime.tier = RunTier::Native;
        record.state = ModelState::Ready;
    };
    let qwen = fixture.register_as(
        "qwen3-embedding.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.pooling_type", 3),
            kv_string("tokenizer.chat_template", "{{ messages }}"),
        ],
        served_as_chat,
    );
    let chat = fixture.register_as("llama.gguf", &llama_chat(), served_as_chat);
    let pinned = fixture.register_as(
        "pinned-embedding.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.pooling_type", 3),
        ],
        |record| {
            served_as_chat(record);
            record.runtime.resolved = Resolution::User;
        },
    );

    let kernel = fixture.kernel();
    let shelf = kernel.shelf().await;
    let qwen = on_shelf(&shelf, &qwen);
    assert_eq!(qwen.state, ModelState::Ready);
    assert_eq!(qwen.runtime.id, Some(RuntimeId::llama_cpp()));
    assert_eq!(qwen.modality, Modality::embedding());
    assert_eq!(qwen.capabilities, vec![Capability::embed()]);
    for id in [&chat, &pinned] {
        let record = on_shelf(&shelf, id);
        assert_eq!(record.state, ModelState::Ready);
        assert!(record.capabilities.contains(&Capability::chat()), "{id}");
    }
}

#[tokio::test]
async fn the_stranded_embedder_migration_runs_once_per_shelf() {
    let fixture = Fixture::new();
    let served_as_chat = |record: &mut ModelRecord| {
        record.modality = Modality::text();
        record.capabilities = vec![Capability::chat(), Capability::complete()];
        record.runtime.id = Some(RuntimeId::llama_cpp());
        record.runtime.resolved = Resolution::Auto;
        record.runtime.tier = RunTier::Native;
        record.state = ModelState::Ready;
    };
    let header = [
        kv_string("general.architecture", "qwen3"),
        kv_u32("qwen3.pooling_type", 3),
        kv_string("tokenizer.chat_template", "{{ messages }}"),
    ];
    let id = fixture.register_as("qwen3-embedding.gguf", &header, served_as_chat);

    let first = fixture.kernel();
    let record = on_shelf(&first.shelf().await, &id);
    assert_eq!(record.capabilities, vec![Capability::embed()]);
    assert_eq!(fixture.registry().migration_level(), 2);

    // The row put back as a chat model: a later process does not look again.
    fixture.register_as("qwen3-embedding.gguf", &header, served_as_chat);
    let later = fixture.kernel();
    let record = on_shelf(&later.shelf().await, &id);
    assert!(record.capabilities.contains(&Capability::chat()));

    // A build that predates the level drops it when it writes the store, so
    // what it registered is migrated again.
    let models = fixture.dirs.sub("registry").join("models.json");
    let mut store: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&models).unwrap()).unwrap();
    store.as_object_mut().unwrap().remove("migration_level");
    std::fs::write(&models, serde_json::to_vec(&store).unwrap()).unwrap();
    let after_downgrade = fixture.kernel();
    let record = on_shelf(&after_downgrade.shelf().await, &id);
    assert_eq!(record.capabilities, vec![Capability::embed()]);
    assert_eq!(fixture.registry().migration_level(), 2);
}

#[tokio::test]
async fn a_chat_gguf_that_is_gone_settles_the_migration_and_one_that_cannot_be_read_defers_it() {
    let served_as_chat = |record: &mut ModelRecord| {
        record.modality = Modality::text();
        record.capabilities = vec![Capability::chat(), Capability::complete()];
        record.runtime.id = Some(RuntimeId::llama_cpp());
        record.runtime.resolved = Resolution::Auto;
        record.runtime.tier = RunTier::Native;
        record.state = ModelState::Ready;
    };

    let deleted = Fixture::new();
    deleted.register_as("deleted-chat.gguf", &llama_chat(), served_as_chat);
    std::fs::remove_file(deleted.root.join("deleted-chat.gguf")).unwrap();
    deleted.kernel().shelf().await;
    assert_eq!(deleted.registry().migration_level(), 2);

    let garbled = Fixture::new();
    garbled.register_as("garbled-chat.gguf", &llama_chat(), served_as_chat);
    std::fs::write(garbled.root.join("garbled-chat.gguf"), b"GGUF\x03").unwrap();
    garbled.kernel().shelf().await;
    assert_eq!(garbled.registry().migration_level(), 0);
    std::fs::write(garbled.root.join("garbled-chat.gguf"), gguf(&llama_chat())).unwrap();
    garbled.kernel().shelf().await;
    assert_eq!(garbled.registry().migration_level(), 2);
}

#[tokio::test]
async fn an_embedder_is_advertised_at_the_most_it_embeds_at_once() {
    let fixture = Fixture::new();
    let embedder = fixture.register(
        "qwen3-embedding.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.context_length", 32768),
            kv_u32("qwen3.pooling_type", 3),
        ],
    );
    let mean = fixture.register(
        "mean-decoder.gguf",
        &[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.context_length", 32768),
            kv_u32("qwen3.pooling_type", 1),
        ],
    );
    let encoder = fixture.register("nomic-embed.gguf", &nomic_embedder());
    let chat = fixture.register("llama.gguf", &llama_chat());

    let kernel = fixture.resolved().await;
    let shelf = kernel.shelf().await;
    let embedder = on_shelf(&shelf, &embedder);
    assert_eq!(embedder.context_length, Some(32768));
    // Launched at 8192, a decoder that pools by the last token takes 8191.
    assert_eq!(kernel.embedding_window(&embedder), Some(8191));
    assert_eq!(
        kernel.embedding_window(&on_shelf(&shelf, &mean)),
        Some(8192)
    );
    assert_eq!(
        kernel.embedding_window(&on_shelf(&shelf, &encoder)),
        Some(2048)
    );
    assert_eq!(kernel.embedding_window(&on_shelf(&shelf, &chat)), None);
}

fn clef_header() -> Vec<Vec<u8>> {
    vec![
        kv_string("general.architecture", "clef"),
        kv_string("clef.decision.type", "clef"),
        kv_u32("clef.context_length", 262_144),
        kv_string("tokenizer.chat_template", "{{ messages }}"),
    ]
}

fn kev_header() -> Vec<Vec<u8>> {
    vec![
        kv_string("general.architecture", "qwen35"),
        kv_string("qwen35.decision.type", "kev"),
        kv_string("tokenizer.chat_template", "{{ messages }}"),
    ]
}

fn laya_header() -> Vec<Vec<u8>> {
    vec![
        kv_string("general.architecture", "modern-bert"),
        kv_string("modern-bert.decision.type", "laya"),
        kv_u32("modern-bert.pooling_type", 1),
    ]
}

/// Shape a row the way v1.4.x left a decision GGUF it took for a chat model.
fn served_as_chat(record: &mut ModelRecord) {
    record.modality = Modality::text();
    record.capabilities = vec![Capability::chat(), Capability::complete()];
    record.runtime.id = Some(RuntimeId::llama_cpp());
    record.runtime.resolved = Resolution::Auto;
    record.runtime.tier = RunTier::Native;
    record.state = ModelState::Ready;
}

/// Mark the store as migrated to `level`, as a build that reached it left it.
fn at_level(fixture: &Fixture, level: u32) {
    let mut registry = fixture.registry();
    let records = registry.records();
    assert!(registry.mark_migrated(level, &records).unwrap());
}

#[tokio::test]
async fn decision_ggufs_an_older_shelf_stranded_are_judges_on_first_sight_of_the_shelf() {
    let fixture = Fixture::new();
    let clef = fixture.register_as("clef-flash.gguf", &clef_header(), served_as_chat);
    let kev = fixture.register_as("kev.gguf", &kev_header(), served_as_chat);
    // v1.4.x took Laya for an embedder it could not serve.
    let laya = fixture.register_as("laya.gguf", &laya_header(), |record| {
        record.modality = Modality::embedding();
        record.capabilities = vec![Capability::embed()];
        record.state = ModelState::Unresolved;
    });
    // A scan after #12, before decision models were served, read Laya as a
    // model that claims nothing.
    let julia = fixture.register_as("julia-1.gguf", &laya_header(), |record| {
        record.modality = Modality::text();
        record.state = ModelState::Unresolved;
    });
    let chat = fixture.register_as("llama.gguf", &llama_chat(), served_as_chat);
    at_level(&fixture, 1);

    // No resolve and no discover: what `hedos serve` does before its first request.
    let shelf = fixture.kernel().shelf().await;
    for id in [&clef, &kev, &laya, &julia] {
        let record = on_shelf(&shelf, id);
        assert_eq!(record.capabilities, vec![Capability::judge()], "{id}");
        assert_eq!(record.modality, Modality::text(), "{id}");
        assert_eq!(record.runtime.id, Some(RuntimeId::llama_cpp()), "{id}");
        assert_eq!(record.state, ModelState::Ready, "{id}");
    }
    let chat = on_shelf(&shelf, &chat);
    assert!(chat.capabilities.contains(&Capability::chat()));
    assert!(!chat.capabilities.contains(&Capability::judge()));
    assert_eq!(fixture.registry().migration_level(), 2);
}

#[tokio::test]
async fn a_decision_gguf_that_cannot_be_read_defers_the_migration_until_it_can() {
    let fixture = Fixture::new();
    let clef = fixture.register_as("clef-flash.gguf", &clef_header(), served_as_chat);
    at_level(&fixture, 1);
    std::fs::write(fixture.root.join("clef-flash.gguf"), b"GGUF\x03").unwrap();

    let shelf = fixture.kernel().shelf().await;
    assert!(
        on_shelf(&shelf, &clef)
            .capabilities
            .contains(&Capability::chat())
    );
    assert_eq!(fixture.registry().migration_level(), 1);

    std::fs::write(fixture.root.join("clef-flash.gguf"), gguf(&clef_header())).unwrap();
    let shelf = fixture.kernel().shelf().await;
    assert_eq!(
        on_shelf(&shelf, &clef).capabilities,
        vec![Capability::judge()]
    );
    assert_eq!(fixture.registry().migration_level(), 2);
}

#[tokio::test]
async fn a_file_the_embedder_migration_cannot_read_never_strands_a_decision_model() {
    // A llama.cpp chat row whose `.gguf` is not a GGUF (a page saved under the
    // name) keeps both levels unrecorded, and the decision pass still serves
    // Laya, which only it re-reads: a scan from before decision models were
    // served left it claiming nothing.
    let fixture = Fixture::new();
    fixture.register_as("saved-page-Q4_K_M.gguf", &llama_chat(), served_as_chat);
    std::fs::write(
        fixture.root.join("saved-page-Q4_K_M.gguf"),
        b"<html>not a model</html>",
    )
    .unwrap();
    let laya = fixture.register_as("laya.gguf", &laya_header(), |record| {
        record.modality = Modality::text();
        record.state = ModelState::Unresolved;
    });

    let shelf = fixture.kernel().shelf().await;
    let laya = on_shelf(&shelf, &laya);
    assert_eq!(laya.capabilities, vec![Capability::judge()]);
    assert_eq!(laya.state, ModelState::Ready);
    assert_eq!(fixture.registry().migration_level(), 0);
}
