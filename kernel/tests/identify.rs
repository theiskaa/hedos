//! Tests for `Identification::identify` across its source-kind and file-layout
//! branches.

mod support;

use std::path::{Path, PathBuf};

use kernel::records::{Capability, ExecutionMode, Modality, ModelRecord, ModelSource, SourceKind};
use kernel::resolution::{IdentifiedModel, ModelFormat, identify, projector_for};
use support::TempDir;

fn record(kind: SourceKind, path: &str) -> ModelRecord {
    ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(kind, path),
    )
}

fn write(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// The snapshot directory a hub-cache repo keeps its named files in. Writing
/// into it creates it, as `write` makes the parents.
fn snapshot_of(dir: &TempDir, revision: &str) -> std::path::PathBuf {
    dir.path().join("snapshots").join(revision)
}

/// A record naming a hub-cache repo directory, with the revision naming the
/// snapshot that is current.
fn hf_record(dir: &TempDir, revision: &str) -> ModelRecord {
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some(revision.to_owned());
    rec
}

#[test]
fn a_builtin_model_has_the_builtin_profile() {
    let id = identify(&record(SourceKind::builtin(), "apple"));
    assert_eq!(id.format, ModelFormat::Builtin);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.execution, ExecutionMode::Stream);
    assert_eq!(id.params.len(), 5);
    assert!(id.params.iter().any(|p| p.key == "temperature"));
}

#[test]
fn an_endpoint_model_has_the_endpoint_profile() {
    let id = identify(&record(SourceKind::endpoint(), "https://api"));
    assert_eq!(id.format, ModelFormat::Endpoint);
    assert_eq!(id.params.len(), 7);
    assert!(id.params.iter().any(|p| p.key == "frequency_penalty"));
    assert!(id.params.iter().any(|p| p.key == "stop"));
}

#[test]
fn an_ollama_chat_model_reads_its_manifest() {
    let dir = TempDir::new();
    let manifest = dir.path().join("manifest");
    write(
        &manifest,
        br#"{"layers":[{"mediaType":"application/vnd.ollama.image.model"}]}"#,
    );
    let mut rec = record(SourceKind::ollama(), manifest.to_str().unwrap());
    // A non-GGUF weight blob → the plain chat profile.
    let blob = dir.path().join("blob");
    write(&blob, b"not-gguf");
    rec.primary_weight_path = Some(blob.to_string_lossy().into_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::OllamaStore);
    assert!(id.capabilities.contains(&Capability::chat()));
    assert!(!id.capabilities.contains(&Capability::see()));
}

#[test]
fn an_ollama_projector_manifest_is_vision() {
    let dir = TempDir::new();
    let manifest = dir.path().join("manifest");
    write(
        &manifest,
        br#"{"layers":[{"mediaType":"x.model"},{"mediaType":"x.projector"}]}"#,
    );
    let id = identify(&record(SourceKind::ollama(), manifest.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::OllamaStore);
    assert!(id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_ggml_bin_is_transcription() {
    let dir = TempDir::new();
    let bin = dir.path().join("whisper.bin");
    write(&bin, b"lmggDATA"); // GGML magic
    let id = identify(&record(SourceKind::file(), bin.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::GgmlBin);
    assert_eq!(id.modality, Some(Modality::audio()));
    assert!(id.capabilities.contains(&Capability::transcribe()));
}

#[test]
fn an_mmproj_gguf_is_vision_only() {
    let dir = TempDir::new();
    let gguf = dir.path().join("mmproj-model.gguf");
    write(&gguf, b"GGUF"); // magic; name marks it a projector
    let id = identify(&record(SourceKind::file(), gguf.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::vision()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_plain_gguf_without_a_known_arch_is_text_chat() {
    let dir = TempDir::new();
    let gguf = dir.path().join("model.gguf");
    write(&gguf, b"GGUF"); // magic but no parseable header → fallback
    let id = identify(&record(SourceKind::file(), gguf.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert!(!id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_gguf_beside_an_mmproj_companion_can_see() {
    let dir = TempDir::new();
    write(&dir.path().join("model.gguf"), b"GGUF");
    write(&dir.path().join("mmproj-model.gguf"), b"GGUF");
    let id = identify(&record(
        SourceKind::file(),
        dir.path().join("model.gguf").to_str().unwrap(),
    ));
    assert!(id.capabilities.contains(&Capability::see()));
}

#[test]
fn a_gguf_carries_the_quantization_its_header_names() {
    let dir = TempDir::new();
    let path = dir.path().join("model.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("general.file_type", 15),
        ]),
    );
    let id = identify(&record(SourceKind::file(), path.to_str().unwrap()));
    assert_eq!(id.quantization.as_deref(), Some("Q4_K_M"));
}

#[test]
fn an_ollama_model_reads_its_quantization_from_the_weight_blob() {
    let dir = TempDir::new();
    let manifest = dir.path().join("manifest");
    write(
        &manifest,
        br#"{"layers":[{"mediaType":"application/vnd.ollama.image.model"}]}"#,
    );
    let blob = dir.path().join("blob");
    write(
        &blob,
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("general.file_type", 7),
        ]),
    );
    let mut rec = record(SourceKind::ollama(), manifest.to_str().unwrap());
    rec.primary_weight_path = Some(blob.to_string_lossy().into_owned());
    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::OllamaStore);
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.quantization.as_deref(), Some("Q8_0"));
}

#[test]
fn an_mlx_folder_names_its_quantization_in_bits() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"quantization":{"bits":4,"group_size":64}}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::MlxSafetensors);
    assert_eq!(id.quantization.as_deref(), Some("4bit"));
}

#[test]
fn a_diffusers_model_index_identifies_as_an_image_job() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"StableDiffusionPipeline"}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Diffusers);
    assert_eq!(id.execution, ExecutionMode::Job);
    assert_eq!(
        id.pipeline_class.as_deref(),
        Some("StableDiffusionPipeline")
    );
    // The pipeline-family registry resolves the class to an image profile.
    assert_eq!(id.modality, Some(Modality::image()));
    assert!(id.capabilities.contains(&Capability::image()));
    assert!(id.params.iter().any(|spec| spec.key == "steps"));
}

#[test]
fn an_unknown_diffusers_class_falls_back_to_a_bare_job() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"MysteryPipeline"}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Diffusers);
    assert_eq!(id.execution, ExecutionMode::Job);
    assert_eq!(id.pipeline_class.as_deref(), Some("MysteryPipeline"));
    assert_eq!(id.modality, None);
    assert!(id.params.is_empty());
}

#[test]
fn a_turbo_scheduler_and_repo_hint_refine_the_params_through_identify() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"StableDiffusionXLPipeline"}"#,
    );
    write(
        &dir.path().join("scheduler").join("scheduler_config.json"),
        br#"{"_class_name":"EulerAncestralDiscreteScheduler","timestep_spacing":"trailing"}"#,
    );
    // The turbo signal comes from `source.repo` (not the name) — exercising the
    // `repo ?? name` repo-hint fallback.
    let mut rec = record(SourceKind::folder(), dir.path().to_str().unwrap());
    rec.source.repo = Some("stabilityai/sdxl-turbo".to_owned());

    let id = identify(&rec);
    let steps = id.params.iter().find(|s| s.key == "steps").unwrap();
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(2))
    );
}

#[test]
fn a_flux_pipeline_without_guidance_embeds_drops_the_guidance_param() {
    let dir = TempDir::new();
    write(
        &dir.path().join("model_index.json"),
        br#"{"_class_name":"FluxPipeline"}"#,
    );
    // No transformer/config.json → guidance is dropped (schnell-style).
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::image()));
    assert!(!id.params.iter().any(|spec| spec.key == "guidance"));
    assert!(id.params.iter().any(|spec| spec.key == "steps"));

    // With guidance_embeds: true, the guidance param stays (dev-style).
    write(
        &dir.path().join("transformer").join("config.json"),
        br#"{"guidance_embeds":true}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert!(id.params.iter().any(|spec| spec.key == "guidance"));
}

#[test]
fn a_safetensors_folder_uses_its_config_hint() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":8192}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn a_sentence_transformers_safetensors_folder_is_embedding() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    write(&dir.path().join("config_sentence_transformers.json"), b"{}");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert_eq!(id.capabilities, vec![Capability::embed()]);
}

#[test]
fn a_cross_encoder_safetensors_folder_is_neither_chat_nor_embedding() {
    let dir = TempDir::new();
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["Qwen3ForCausalLM"]}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    write(
        &dir.path().join("config_sentence_transformers.json"),
        br#"{"model_type":"CrossEncoder"}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_config_without_weights_falls_to_unknown_with_the_hint() {
    let dir = TempDir::new();
    // A recognized embedding arch but no safetensors on disk.
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["BertModel"]}"#,
    );
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert_eq!(id.modality, Some(Modality::embedding()));
}

#[test]
fn an_unrecognized_folder_is_unknown() {
    let dir = TempDir::new();
    write(&dir.path().join("README.md"), b"hi");
    let id = identify(&record(SourceKind::folder(), dir.path().to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert_eq!(id.modality, None);
    assert!(id.capabilities.is_empty());
    assert_eq!(id.execution, ExecutionMode::Sync);
}

// A minimal GGUF header builder (mirrors the one in resolution.rs) so identify()
// can exercise the recognized-architecture branch.
fn gguf_string(value: &str) -> Vec<u8> {
    let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

fn kv_string(key: &str, value: &str) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend(gguf_string(value));
    bytes
}

fn kv_u32(key: &str, value: u32) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

fn gguf(kvs: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    for kv in kvs {
        bytes.extend_from_slice(kv);
    }
    bytes
}

#[test]
fn a_recognized_gguf_architecture_uses_its_profile_and_facts() {
    let dir = TempDir::new();
    // Whisper arch → audio/transcription.
    let whisper = dir.path().join("whisper.gguf");
    write(
        &whisper,
        &gguf(&[kv_string("general.architecture", "whisper")]),
    );
    let id = identify(&record(SourceKind::file(), whisper.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::audio()));
    assert!(id.capabilities.contains(&Capability::transcribe()));

    // A llama arch with a context length and chat template → text chat + facts.
    let llama = dir.path().join("llama.gguf");
    write(
        &llama,
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
            kv_string("tokenizer.chat_template", "{{ x }}"),
        ]),
    );
    let id = identify(&record(SourceKind::file(), llama.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.context_length, Some(4096));
    assert_eq!(id.has_chat_template, Some(true));
}

fn encoder(dir: &TempDir, name: &str, architecture: &str, pooling: Option<u32>) -> ModelRecord {
    let path = dir.path().join(name);
    let mut kvs = vec![kv_string("general.architecture", architecture)];
    if let Some(pooling) = pooling {
        kvs.push(kv_u32(&format!("{architecture}.pooling_type"), pooling));
    }
    write(&path, &gguf(&kvs));
    record(SourceKind::file(), path.to_str().unwrap())
}

#[test]
fn a_bert_gguf_with_mean_pooling_embeds() {
    let dir = TempDir::new();
    let id = identify(&encoder(&dir, "mean.gguf", "bert", Some(1)));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert_eq!(id.capabilities, vec![Capability::embed()]);
}

#[test]
fn a_bert_gguf_with_rank_pooling_is_a_reranker_with_no_capability() {
    let dir = TempDir::new();
    let id = identify(&encoder(&dir, "rank.gguf", "bert", Some(4)));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn an_encoder_gguf_with_no_pooling_serves_nothing() {
    let dir = TempDir::new();
    let id = identify(&encoder(&dir, "none.gguf", "t5encoder", Some(0)));
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn an_encoder_gguf_that_names_no_pooling_serves_nothing() {
    // llama.cpp falls back to no pooling when the header names none, and its
    // `/v1/embeddings` refuses a model that pools nothing.
    let dir = TempDir::new();
    let id = identify(&encoder(&dir, "default.gguf", "nomic-bert", None));
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert!(id.capabilities.is_empty());

    let unknown = identify(&encoder(&dir, "unknown.gguf", "nomic-bert", Some(9)));
    assert!(unknown.capabilities.is_empty());
}

fn decision_gguf(dir: &TempDir, name: &str, architecture: &str, decision: &str) -> PathBuf {
    let path = dir.path().join(name);
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", architecture),
            kv_string(&format!("{architecture}.decision.type"), decision),
            kv_u32(&format!("{architecture}.context_length"), 262_144),
            kv_string("tokenizer.chat_template", "{{ x }}"),
        ]),
    );
    path
}

fn identify_file(path: &Path) -> IdentifiedModel {
    identify(&record(SourceKind::file(), path.to_str().unwrap()))
}

#[test]
fn an_encoder_gguf_with_a_decision_head_is_a_judge_and_not_an_embedder() {
    // Laya and Julia-1: a `modern-bert` encoder whose classifier pools, plus a
    // decision head that is what llama.cpp serves.
    let dir = TempDir::new();
    let path = dir.path().join("laya.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "modern-bert"),
            kv_string("modern-bert.decision.type", "laya"),
            kv_u32("modern-bert.classifier.pooling_type", 1),
            kv_u32("modern-bert.pooling_type", 1),
            kv_u32("modern-bert.context_length", 8192),
        ]),
    );
    let id = identify_file(&path);
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert_eq!(id.capabilities, vec![Capability::judge()]);
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn a_decision_gguf_is_a_judge_whatever_its_architecture_and_chat_template() {
    // clef is an architecture of its own; OpenJev, Kev and lev are `qwen35`, a
    // chat architecture, with a chat template. The decision key wins over both.
    let dir = TempDir::new();
    for (architecture, decision) in [
        ("clef", "clef"),
        ("qwen35", "openjev"),
        ("qwen35", "kev"),
        ("qwen35", "lev"),
    ] {
        let path = decision_gguf(&dir, &format!("{decision}.gguf"), architecture, decision);
        let id = identify_file(&path);
        assert_eq!(id.modality, Some(Modality::text()), "{decision}");
        assert_eq!(id.capabilities, vec![Capability::judge()], "{decision}");
        assert_eq!(id.execution, ExecutionMode::Stream, "{decision}");
        assert_eq!(id.context_length, Some(262_144), "{decision}");
        assert_eq!(id.has_chat_template, Some(true), "{decision}");
    }
}

#[test]
fn a_decision_gguf_sees_only_when_its_type_reads_images_and_a_projector_came_with_it() {
    for (architecture, decision, sees) in [
        ("clef", "clef", true),
        ("qwen35", "openjev", true),
        ("qwen35", "kev", false),
        ("qwen35", "lev", false),
        ("modern-bert", "laya", false),
    ] {
        let dir = TempDir::new();
        let path = decision_gguf(&dir, "model-Q4_K_M.gguf", architecture, decision);
        write(&dir.path().join("mmproj-model-Q8_0.gguf"), b"GGUF");
        let mut expected = vec![Capability::judge()];
        if sees {
            expected.push(Capability::see());
        }
        assert_eq!(identify_file(&path).capabilities, expected, "{decision}");
    }
}

#[test]
fn a_decision_gguf_without_a_projector_never_sees() {
    let dir = TempDir::new();
    let path = decision_gguf(&dir, "clef.gguf", "clef", "clef");
    assert_eq!(identify_file(&path).capabilities, vec![Capability::judge()]);
}

#[test]
fn a_decision_key_named_for_another_architecture_is_not_a_decision_model() {
    let dir = TempDir::new();
    let path = dir.path().join("qwen35.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "qwen35"),
            kv_string("clef.decision.type", "clef"),
            kv_string("tokenizer.chat_template", "{{ x }}"),
        ]),
    );
    assert_eq!(
        identify_file(&path).capabilities,
        vec![Capability::chat(), Capability::complete()]
    );
}

#[test]
fn a_decoder_gguf_whose_header_pools_is_an_embedder_and_not_a_chat_model() {
    // Qwen3-Embedding: a decoder architecture with a chat template, converted
    // to embed with last-token pooling.
    let dir = TempDir::new();
    let path = dir.path().join("qwen3-embedding.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "qwen3"),
            kv_u32("qwen3.context_length", 32768),
            kv_u32("qwen3.pooling_type", 3),
            kv_string("tokenizer.chat_template", "{{ x }}"),
        ]),
    );
    let id = identify(&record(SourceKind::file(), path.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::embedding()));
    assert_eq!(id.capabilities, vec![Capability::embed()]);
    assert_eq!(id.context_length, Some(32768));

    for pooling in [1, 2] {
        let id = identify(&encoder(&dir, "llama-pooled.gguf", "llama", Some(pooling)));
        assert_eq!(
            id.capabilities,
            vec![Capability::embed()],
            "pooling {pooling}"
        );
    }
}

#[test]
fn a_decoder_gguf_that_ranks_claims_nothing() {
    // Qwen3-Reranker: rank pooling under the decoder's own architecture.
    let dir = TempDir::new();
    let id = identify(&encoder(&dir, "qwen3-reranker.gguf", "qwen3", Some(4)));
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_decoder_gguf_that_pools_nothing_stays_a_chat_model() {
    let dir = TempDir::new();
    for pooling in [None, Some(0), Some(9)] {
        let id = identify(&encoder(&dir, "chat.gguf", "llama", pooling));
        assert_eq!(id.modality, Some(Modality::text()), "pooling {pooling:?}");
        assert_eq!(
            id.capabilities,
            vec![Capability::chat(), Capability::complete()],
            "pooling {pooling:?}"
        );
    }
}

#[test]
fn a_decision_gguf_never_embeds_whatever_its_header_pools() {
    let dir = TempDir::new();
    let path = dir.path().join("clef.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "clef"),
            kv_string("clef.decision.type", "clef"),
            kv_u32("clef.pooling_type", 3),
        ]),
    );
    let id = identify(&record(SourceKind::file(), path.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::text()));
    assert_eq!(id.capabilities, vec![Capability::judge()]);
}

#[test]
fn an_encoder_gguf_with_a_classifier_head_and_no_pooling_reads_as_a_reranker() {
    // bge-reranker-v2-m3 and jina-reranker GGUFs name no pooling but carry the
    // classifier head they score a pair with.
    let dir = TempDir::new();
    let path = dir.path().join("reranker.gguf");
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend(kv_string("general.architecture", "bert"));
    bytes.extend(gguf_string("cls.weight"));
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&1024u64.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    write(&path, &bytes);
    let id = identify(&record(SourceKind::file(), path.to_str().unwrap()));
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn an_encoder_gguf_never_takes_pooling_named_for_another_architecture() {
    let dir = TempDir::new();
    let path = dir.path().join("wrongarch.gguf");
    write(
        &path,
        &gguf(&[
            kv_string("general.architecture", "bert"),
            kv_u32("nomic-bert.pooling_type", 1),
        ]),
    );
    let id = identify(&record(SourceKind::file(), path.to_str().unwrap()));
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_gguf_magic_file_without_the_extension_is_still_gguf() {
    let dir = TempDir::new();
    // No `.gguf` extension, but the GGUF magic → identified by magic.
    let weights = dir.path().join("model.weights");
    write(
        &weights,
        &gguf(&[kv_string("general.architecture", "whisper")]),
    );
    let id = identify(&record(SourceKind::file(), weights.to_str().unwrap()));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::audio()));
}

#[test]
fn a_missing_snapshot_ref_falls_back_to_the_base_container() {
    let dir = TempDir::new();
    // config.json sits at the base; the ref names a snapshot that isn't present.
    write(
        &dir.path().join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&dir.path().join("model.safetensors"), b"weights");
    let mut rec = record(
        SourceKind::huggingface_cache(),
        dir.path().to_str().unwrap(),
    );
    rec.source.reference = Some("does-not-exist".to_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
}

#[test]
fn a_hugging_face_record_resolves_its_snapshot_container() {
    let dir = TempDir::new();
    // The config lives under snapshots/<ref>/, not at the base.
    let snapshot = snapshot_of(&dir, "rev1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["MistralForCausalLM"]}"#,
    );
    write(&snapshot.join("model.safetensors"), b"weights");

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_eq!(id.modality, Some(Modality::text()));
}

#[test]
fn a_snapshot_of_gguf_weights_is_a_gguf_model() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    let weights = snapshot.join("qwen2.5-0.5b-instruct-q4_k_m.gguf");
    write(
        &weights,
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
        ]),
    );
    write(&snapshot.join("LICENSE"), b"terms");
    let mut rec = hf_record(&dir, "rev1");
    // The scanner names the weights it would have a runtime load.
    rec.primary_weight_path = Some(weights.to_string_lossy().into_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert_eq!(id.context_length, Some(4096));
    assert_eq!(id.execution, ExecutionMode::Stream);
}

#[cfg(unix)]
#[test]
fn weights_are_read_and_measured_through_the_snapshot_symlink() {
    let dir = TempDir::new();
    let blob = dir.path().join("blobs").join("9ee3aa1b");
    let mut bytes = gguf(&[
        kv_string("general.architecture", "llama"),
        kv_u32("llama.context_length", 4096),
    ]);
    bytes.resize(bytes.len() + 800, 0);
    write(&blob, &bytes);
    let snapshot = snapshot_of(&dir, "rev1");
    // Larger than the link itself, smaller than the weights it points at:
    // measuring the link rather than its target would pick this one, whose
    // header says nothing.
    write(&snapshot.join("decoy.gguf"), &b"GGUF".repeat(50));
    std::os::unix::fs::symlink("../../blobs/9ee3aa1b", snapshot.join("model.gguf")).unwrap();
    let mut rec = hf_record(&dir, "rev1");
    rec.primary_weight_path = Some(blob.to_string_lossy().into_owned());

    let id = identify(&rec);
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(
        id.context_length,
        Some(4096),
        "the linked weights were read"
    );
}

#[test]
fn a_projector_beside_the_weights_adds_sight_but_is_not_the_weights() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(&snapshot.join("model.gguf"), b"GGUF");
    // Larger than the model, and still not the model.
    write(&snapshot.join("mmproj-model-f16.gguf"), &b"GGUF".repeat(20));

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
    assert!(id.capabilities.contains(&Capability::see()));
}

/// A loose GGUF record for `weights` beside each projector in `projectors`
/// (name, size), quantized as `quantization`.
fn loose_with_projectors(
    dir: &TempDir,
    weights: &str,
    quantization: Option<&str>,
    projectors: &[(&str, usize)],
) -> ModelRecord {
    let path = dir.path().join(weights);
    write(&path, b"GGUF");
    for (name, size) in projectors {
        write(&dir.path().join(name), &projector(64, *size));
    }
    let mut rec = record(SourceKind::file(), path.to_str().unwrap());
    rec.quantization = quantization.map(str::to_owned);
    rec
}

fn file_name(path: Option<PathBuf>) -> Option<String> {
    path.and_then(|path| path.file_name()?.to_str().map(str::to_owned))
}

#[test]
fn the_projector_named_for_the_weights_quantization_is_the_one_served() {
    let dir = TempDir::new();
    let rec = loose_with_projectors(
        &dir,
        "Clef-Flash-Q8_0.gguf",
        Some("Q8_0"),
        &[
            ("mmproj-Clef-Flash-BF16.gguf", 90),
            ("mmproj-Clef-Flash-Q8_0.gguf", 60),
        ],
    );
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-Clef-Flash-Q8_0.gguf")
    );

    // BF16 weights take the BF16 projector, though it is the larger one.
    let dir = TempDir::new();
    let rec = loose_with_projectors(
        &dir,
        "model-BF16.gguf",
        Some("BF16"),
        &[
            ("mmproj-model-BF16.gguf", 90),
            ("mmproj-model-Q8_0.gguf", 60),
        ],
    );
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-model-BF16.gguf")
    );
}

#[test]
fn weights_with_no_projector_of_their_quantization_take_the_smallest() {
    let dir = TempDir::new();
    let rec = loose_with_projectors(
        &dir,
        "Clef-Flash-Q4_K_M.gguf",
        Some("Q4_K_M"),
        &[
            ("mmproj-Clef-Flash-BF16.gguf", 90),
            ("mmproj-Clef-Flash-Q8_0.gguf", 60),
        ],
    );
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-Clef-Flash-Q8_0.gguf")
    );

    // A quantization only part of a name is not a match: F16 weights do not
    // take the BF16 projector for it, but the smaller one.
    let dir = TempDir::new();
    let rec = loose_with_projectors(
        &dir,
        "model-F16.gguf",
        Some("F16"),
        &[
            ("mmproj-model-BF16.gguf", 90),
            ("mmproj-model-Q8_0.gguf", 60),
        ],
    );
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-model-Q8_0.gguf")
    );

    let dir = TempDir::new();
    let rec = loose_with_projectors(
        &dir,
        "model.gguf",
        None,
        &[("mmproj-b.gguf", 60), ("mmproj-a.gguf", 60)],
    );
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-a.gguf")
    );
}

/// A GGUF header of `architecture` with `width` as its embedding width.
fn weights_of_width(architecture: &str, width: u32) -> Vec<u8> {
    gguf(&[
        kv_string("general.architecture", architecture),
        kv_u32(&format!("{architecture}.embedding_length"), width),
    ])
}

/// A vision projector: its header marks an image encoder and lists one
/// tensor `width` wide, whose data follows, `padding` bytes and one more.
fn projector(width: u64, padding: usize) -> Vec<u8> {
    projector_with(width, padding, true)
}

/// [`projector`], with `vision` saying whether it encodes images.
fn projector_with(width: u64, padding: usize, vision: bool) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    bytes.extend(kv_string("general.architecture", "clip"));
    let encoder = if vision {
        "clip.has_vision_encoder"
    } else {
        "clip.has_audio_encoder"
    };
    bytes.extend(gguf_string(encoder));
    bytes.extend_from_slice(&7u32.to_le_bytes());
    bytes.push(1);
    bytes.extend(gguf_string("mm.2.weight"));
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.resize(bytes.len().div_ceil(32) * 32 + padding + 1, 0);
    bytes
}

fn projector_of_width(width: u64) -> Vec<u8> {
    projector(width, 0)
}

#[test]
fn another_models_projector_in_the_same_folder_is_not_this_ones_sight() {
    // A text model kept in ~/Models beside gemma and gemma's projector:
    // llama.cpp refuses a projector whose width is not the model's.
    let dir = TempDir::new();
    let llama = dir.path().join("Llama-3.2-3B-Instruct-Q4_K_M.gguf");
    write(&llama, &weights_of_width("llama", 3072));
    let gemma = dir.path().join("gemma-3-4b-it-Q4_K_M.gguf");
    write(&gemma, &weights_of_width("gemma3", 2560));
    write(
        &dir.path().join("mmproj-gemma-3-4b-it-f16.gguf"),
        &projector_of_width(2560),
    );

    let llama_id = identify_file(&llama);
    assert_eq!(
        llama_id.capabilities,
        vec![Capability::chat(), Capability::complete()]
    );
    let mut llama_record = record(SourceKind::file(), llama.to_str().unwrap());
    llama_record.capabilities = llama_id.capabilities;
    assert_eq!(projector_for(&llama_record), None);

    let gemma_id = identify_file(&gemma);
    assert!(gemma_id.capabilities.contains(&Capability::see()));
    let gemma_record = record(SourceKind::file(), gemma.to_str().unwrap());
    assert_eq!(
        file_name(projector_for(&gemma_record)).as_deref(),
        Some("mmproj-gemma-3-4b-it-f16.gguf")
    );
}

#[test]
fn an_audio_only_projector_is_not_sight() {
    let dir = TempDir::new();
    let model = dir.path().join("ultravox-Q4_K_M.gguf");
    write(&model, &weights_of_width("llama", 3072));
    write(
        &dir.path().join("mmproj-ultravox-f16.gguf"),
        &projector_with(3072, 0, false),
    );
    assert!(
        !identify_file(&model)
            .capabilities
            .contains(&Capability::see())
    );
    let rec = record(SourceKind::file(), model.to_str().unwrap());
    assert_eq!(projector_for(&rec), None);
}

#[test]
fn an_older_converters_projector_is_matched_by_its_tensors_not_its_header() {
    // llava 1.6's projector names CLIP's 768 as its projection width; its
    // output tensor is the 4096 the text model takes, which is what llama.cpp
    // checks.
    let dir = TempDir::new();
    let model = dir.path().join("llava-v1.6-mistral-7b.Q4_K_M.gguf");
    write(&model, &weights_of_width("llama", 4096));
    write(
        &dir.path().join("mmproj-model-f16.gguf"),
        &projector(4096, 0),
    );
    assert!(
        identify_file(&model)
            .capabilities
            .contains(&Capability::see())
    );
    let rec = record(SourceKind::file(), model.to_str().unwrap());
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-model-f16.gguf")
    );
}

#[test]
fn a_projector_cut_short_or_unreadable_is_never_loaded_but_one_beside_it_is() {
    let dir = TempDir::new();
    let model = dir.path().join("SmolVLM2-Q8_0.gguf");
    write(&model, &weights_of_width("llama", 960));
    let mut cut = projector(960, 0);
    cut.truncate(cut.len() - 1);
    write(&dir.path().join("mmproj-SmolVLM2-a-f32.gguf"), &cut);
    write(&dir.path().join("mmproj-SmolVLM2-b-f16.gguf"), b"GGUF");
    write(
        &dir.path().join("mmproj-SmolVLM2-c-Q8_0.gguf"),
        &projector(960, 4096),
    );
    let rec = record(SourceKind::file(), model.to_str().unwrap());
    assert_eq!(
        file_name(projector_for(&rec)).as_deref(),
        Some("mmproj-SmolVLM2-c-Q8_0.gguf")
    );

    // With only those beside it, the model still says it sees, as a name is
    // all there is to go on, but no server is given either.
    std::fs::remove_file(dir.path().join("mmproj-SmolVLM2-c-Q8_0.gguf")).unwrap();
    assert!(
        identify_file(&model)
            .capabilities
            .contains(&Capability::see())
    );
    assert_eq!(projector_for(&rec), None);
}

#[test]
fn a_decision_model_beside_another_models_projector_does_not_see() {
    let dir = TempDir::new();
    let openjev = dir.path().join("OpenJev-Q4_K_M.gguf");
    write(
        &openjev,
        &gguf(&[
            kv_string("general.architecture", "qwen35"),
            kv_string("qwen35.decision.type", "openjev"),
            kv_u32("qwen35.embedding_length", 5120),
        ]),
    );
    write(
        &dir.path().join("mmproj-Clef-Flash-Q8_0.gguf"),
        &projector_of_width(4096),
    );
    assert_eq!(
        identify_file(&openjev).capabilities,
        vec![Capability::judge()]
    );
}

#[test]
fn a_snapshots_projector_of_another_width_is_not_its_sight() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(
        &snapshot.join("model.gguf"),
        &weights_of_width("llama", 3072),
    );
    write(
        &snapshot.join("mmproj-other.gguf"),
        &projector_of_width(2560),
    );
    let rec = hf_record(&dir, "rev1");
    assert!(!identify(&rec).capabilities.contains(&Capability::see()));
    assert_eq!(projector_for(&rec), None);
}

#[test]
fn a_gguf_with_no_projector_or_from_ollama_has_none_to_serve() {
    let dir = TempDir::new();
    let rec = loose_with_projectors(&dir, "model.gguf", Some("Q8_0"), &[]);
    assert_eq!(projector_for(&rec), None);

    let dir = TempDir::new();
    write(&dir.path().join("mmproj-model.gguf"), b"GGUF");
    let ollama = record(
        SourceKind::ollama(),
        dir.path().join("manifest").to_str().unwrap(),
    );
    assert_eq!(projector_for(&ollama), None);
}

#[test]
fn a_snapshots_projector_is_found_where_its_sight_was_and_served_by_its_snapshot_path() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(&snapshot.join("Q4_K_M").join("model-Q4_K_M.gguf"), b"GGUF");
    write(&snapshot.join("mmproj-model-BF16.gguf"), &projector(64, 80));
    write(&snapshot.join("mmproj-model-Q8_0.gguf"), &projector(64, 40));
    let mut rec = hf_record(&dir, "rev1");
    rec.quantization = Some("Q4_K_M".to_owned());

    assert!(identify(&rec).capabilities.contains(&Capability::see()));
    assert_eq!(
        projector_for(&rec),
        Some(snapshot.join("mmproj-model-Q8_0.gguf"))
    );
}

#[test]
fn a_snapshot_holding_only_a_projector_is_not_a_model() {
    let dir = TempDir::new();
    write(
        &snapshot_of(&dir, "rev1").join("mmproj-model-f16.gguf"),
        b"GGUF",
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert!(id.capabilities.is_empty());
}

#[test]
fn a_config_and_safetensors_layout_still_wins_over_gguf_weights_beside_it() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("model.safetensors"), b"weights");
    write(&snapshot.join("extra.gguf"), b"GGUF");

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Safetensors);
}

#[test]
fn gguf_weights_outrank_a_config_with_nothing_else_to_read() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    // A repo that ships the config of the model it was quantized from: the
    // header is the better of the two, and the only one a server can load.
    write(
        &snapshot.join("config.json"),
        br#"{"architectures":["LlamaForCausalLM"]}"#,
    );
    write(&snapshot.join("model.GGUF"), b"GGUF");

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert!(id.capabilities.contains(&Capability::chat()));
}

#[test]
fn a_sharded_snapshot_is_identified_from_its_first_shard() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    // Only the first shard carries the metadata, and it is not the largest.
    write(
        &snapshot.join("model-00001-of-00002.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 8192),
        ]),
    );
    write(
        &snapshot.join("model-00002-of-00002.gguf"),
        &b"GGUF".repeat(100),
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn weights_kept_in_a_quantization_directory_are_still_the_model() {
    let dir = TempDir::new();
    // The layout a repo shipping several quantizations uses: nothing at the
    // snapshot root, one directory per quantization below it.
    write(
        &snapshot_of(&dir, "rev1").join("Q4_K_M").join("model.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 8192),
        ]),
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn a_shard_set_is_read_through_the_first_file_beside_it_not_one_a_folder_away() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    // Two quantizations of the same model, so both sets share a base name.
    // The larger set is the Q8 one, and its own first file is what its header
    // must be read from.
    for (quantization, window, tail) in [("Q4_K_M", 4096u32, 100), ("Q8_0", 8192, 400)] {
        write(
            &snapshot
                .join(quantization)
                .join("model-00001-of-00002.gguf"),
            &gguf(&[
                kv_string("general.architecture", "llama"),
                kv_u32("llama.context_length", window),
            ]),
        );
        write(
            &snapshot
                .join(quantization)
                .join("model-00002-of-00002.gguf"),
            &b"GGUF".repeat(tail),
        );
    }

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(
        id.context_length,
        Some(8192),
        "the first file of the set the largest shard belongs to"
    );
}

#[test]
fn an_architecture_that_can_see_does_not_without_an_image_encoder() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    let weights = gguf(&[kv_string("general.architecture", "qwen2vl")]);
    write(&snapshot.join("model.gguf"), &weights);

    let blind = identify(&hf_record(&dir, "rev1"));
    assert!(blind.capabilities.contains(&Capability::chat()));
    assert!(
        !blind.capabilities.contains(&Capability::see()),
        "no projector came with these weights: {:?}",
        blind.capabilities
    );

    // The projector arrives, and with it the sight, even from the snapshot
    // root while the weights sit a directory down.
    let deep = TempDir::new();
    let below = snapshot_of(&deep, "rev1");
    write(&below.join("Q4_K_M").join("model.gguf"), &weights);
    write(&below.join("mmproj-model-f16.gguf"), b"GGUF");
    let seeing = identify(&hf_record(&deep, "rev1"));
    assert!(seeing.capabilities.contains(&Capability::see()));
}

#[test]
fn a_half_downloaded_set_is_passed_over_for_the_weights_that_are_whole() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    // The bigger quantization is still coming down and has no first file, so
    // nothing can be loaded through it. The smaller one is whole.
    write(
        &snapshot.join("model.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
        ]),
    );
    write(
        &snapshot.join("Q8_0").join("model-00002-of-00002.gguf"),
        &b"GGUF".repeat(400),
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.context_length, Some(4096));
}

#[test]
fn a_shard_set_without_its_first_file_is_not_a_model() {
    let dir = TempDir::new();
    // A half-downloaded set: a server loads the rest through the first file,
    // so without it there is nothing to serve.
    write(
        &snapshot_of(&dir, "rev1").join("model-00002-of-00002.gguf"),
        b"GGUF",
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Unknown);
}

#[test]
fn a_directory_wearing_the_name_of_a_weight_is_not_one() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    std::fs::create_dir_all(snapshot.join("quantized.gguf")).unwrap();

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Unknown);
}

#[test]
fn weights_the_runtime_would_not_load_are_not_read_as_gguf() {
    let dir = TempDir::new();
    write(&snapshot_of(&dir, "rev1").join("model.gguf"), b"GGUF");
    let elsewhere = dir.path().join("elsewhere.bin");
    write(&elsewhere, b"not-a-gguf");

    let mut rec = hf_record(&dir, "rev1");
    rec.primary_weight_path = Some(elsewhere.to_string_lossy().into_owned());
    assert_eq!(identify(&rec).format, ModelFormat::Unknown);
}

#[test]
fn weights_that_are_gone_are_not_read_as_gguf() {
    let dir = TempDir::new();
    write(&snapshot_of(&dir, "rev1").join("model.gguf"), b"GGUF");

    // What a swept blob leaves behind: nothing to read is nothing to serve.
    let mut rec = hf_record(&dir, "rev1");
    rec.primary_weight_path = Some(dir.path().join("swept").to_string_lossy().into_owned());
    assert_eq!(identify(&rec).format, ModelFormat::Unknown);
}

#[test]
fn equal_weights_are_read_the_same_way_every_time() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    // The same size, so only the name can decide which is read.
    write(
        &snapshot.join("b-second.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 2048),
        ]),
    );
    write(
        &snapshot.join("a-first.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
        ]),
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.context_length, Some(4096), "the earlier name wins a tie");
}

#[test]
fn a_hidden_leftover_is_not_taken_for_the_weights() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(
        &snapshot.join("model.gguf"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 4096),
        ]),
    );
    // Larger, and hidden, which is how the scanners read a leftover too.
    write(&snapshot.join(".old-model.gguf"), &b"GGUF".repeat(200));

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.context_length, Some(4096));
}

#[test]
fn a_sharded_snapshot_spelled_in_capitals_is_still_a_model() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    write(
        &snapshot.join("MODEL-00001-of-00002.GGUF"),
        &gguf(&[
            kv_string("general.architecture", "llama"),
            kv_u32("llama.context_length", 8192),
        ]),
    );
    write(
        &snapshot.join("MODEL-00002-of-00002.GGUF"),
        &b"GGUF".repeat(100),
    );

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Gguf);
    assert_eq!(id.context_length, Some(8192));
}

#[test]
fn a_shard_set_whose_first_file_is_a_directory_is_not_a_model() {
    let dir = TempDir::new();
    let snapshot = snapshot_of(&dir, "rev1");
    std::fs::create_dir_all(snapshot.join("model-00001-of-00002.gguf")).unwrap();
    write(&snapshot.join("model-00002-of-00002.gguf"), b"GGUF");

    let id = identify(&hf_record(&dir, "rev1"));
    assert_eq!(id.format, ModelFormat::Unknown);
}
