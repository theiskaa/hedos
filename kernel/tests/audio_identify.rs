//! How audio generators and voices are told apart, from the files their
//! repos publish. The fixtures under `fixtures/audio/` are each repo's real
//! `config.json` or `model_index.json` and the front matter of its model card,
//! as Hugging Face served them on 2026-10-11 (no weights). Two repos are gated:
//! `csm-1b` and `orpheus-3b` hold the architecture, model type and card fields
//! the Hub API reports for them, written out by hand. `decomposer-qwen3-8b`
//! ships a chat template, stood in for by a short one.

mod support;

use std::path::{Path, PathBuf};

use kernel::discovery::modality_hints::from_snapshot;
use kernel::discovery::{HFCacheScanner, StoreScanner};
use kernel::records::{Capability, ExecutionMode, Modality, ModelRecord, ModelSource, SourceKind};
use kernel::resolution::{IdentifiedModel, ModelFormat, identify};
use support::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// A folder holding `name`'s fixture files and, unless `weights` is false, a
/// stub `model.safetensors` so it reads as a safetensors checkpoint.
fn folder(name: &str, weights: bool) -> TempDir {
    let dir = TempDir::new();
    for entry in std::fs::read_dir(fixture(name)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    if weights {
        std::fs::write(dir.path().join("model.safetensors"), b"weights").unwrap();
    }
    dir
}

fn identified(dir: &TempDir) -> IdentifiedModel {
    identify(&ModelRecord::new(
        "m",
        Modality::unknown(),
        Vec::new(),
        ModelSource::new(SourceKind::folder(), dir.path().to_str().unwrap()),
    ))
}

fn assert_withheld(id: &IdentifiedModel, modality: Modality) {
    assert_eq!(id.modality, Some(modality));
    assert!(
        id.capabilities.is_empty(),
        "claims {:?}, but nothing built in drives it",
        id.capabilities
    );
}

#[test]
fn yue2_makes_audio_and_never_chats() {
    let id = identified(&folder("yue2-3b", true));
    assert_eq!(id.format, ModelFormat::Safetensors);
    assert_withheld(&id, Modality::audio());
    assert_eq!(id.execution, ExecutionMode::Job);
}

#[test]
fn an_mlx_conversion_of_yue2_is_still_no_chat_model() {
    let dir = folder("yue2-3b", true);
    let config = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    let quantized = config.replacen('{', r#"{"quantization":{"bits":8,"group_size":64},"#, 1);
    std::fs::write(dir.path().join("config.json"), quantized).unwrap();
    let id = identified(&dir);
    assert_eq!(id.format, ModelFormat::MlxSafetensors);
    assert_withheld(&id, Modality::audio());
    assert_eq!(id.quantization.as_deref(), Some("8bit"));
}

#[test]
fn yue2s_decoder_is_a_part_not_a_chat_model() {
    assert_withheld(&identified(&folder("yue2-vae", true)), Modality::audio());
}

#[test]
fn custom_package_generators_are_withheld() {
    for name in ["ace-step-1.5", "heartmula-3b"] {
        let id = identified(&folder(name, true));
        assert_withheld(&id, Modality::audio());
        assert_eq!(id.execution, ExecutionMode::Job, "{name}");
    }
}

#[test]
fn musicgen_is_served_as_an_audio_job_with_its_params() {
    let id = identified(&folder("musicgen-small", true));
    assert_eq!(id.modality, Some(Modality::audio()));
    assert_eq!(id.capabilities, vec![Capability::audio()]);
    assert_eq!(id.execution, ExecutionMode::Job);
    let keys: Vec<&str> = id.params.iter().map(|spec| spec.key.as_str()).collect();
    assert_eq!(keys, ["duration", "guidance", "seed"]);
}

#[test]
fn diffusers_audio_pipelines_generate_audio() {
    let ldm = identified(&folder("audioldm2-music", false));
    assert_eq!(ldm.format, ModelFormat::Diffusers);
    assert_eq!(ldm.capabilities, vec![Capability::audio()]);
    assert!(ldm.params.iter().any(|spec| spec.key == "duration"));
    assert!(!ldm.params.iter().any(|spec| spec.key == "lyrics"));

    let ace = identified(&folder("ace-step-xl-turbo-diffusers", false));
    assert_eq!(ace.capabilities, vec![Capability::audio()]);
    assert_eq!(ace.pipeline_class.as_deref(), Some("AceStepPipeline"));
    assert!(ace.params.iter().any(|spec| spec.key == "lyrics"));
}

#[test]
fn a_music_tagged_llama_is_not_a_chat_model() {
    // YuE v1's stage-1 model: a plain Llama checkpoint filed as text-generation,
    // tagged music. It writes audio tokens, never a reply.
    assert_withheld(&identified(&folder("yue-s1-7b", true)), Modality::audio());
}

#[test]
fn kokoro_stays_a_voice() {
    let id = identified(&folder("kokoro-82m-bf16", true));
    assert_eq!(id.modality, Some(Modality::speech()));
    assert_eq!(id.capabilities, vec![Capability::speak()]);
}

#[test]
fn kokoro_stays_a_voice_whatever_its_card_says() {
    let dir = folder("kokoro-82m-bf16", true);
    std::fs::write(
        dir.path().join("README.md"),
        "---\npipeline_tag: text-to-audio\ntags:\n- music\n---\n",
    )
    .unwrap();
    let id = identified(&dir);
    assert_eq!(id.modality, Some(Modality::speech()));
    assert_eq!(id.capabilities, vec![Capability::speak()]);
}

#[test]
fn bark_is_a_voice_though_its_card_also_says_text_to_audio() {
    let id = identified(&folder("bark", true));
    assert_eq!(id.modality, Some(Modality::speech()));
    assert_eq!(id.capabilities, vec![Capability::speak()]);
}

#[test]
fn a_tts_card_withholds_chat_from_a_causal_lm_voice() {
    // Orpheus is a Llama checkpoint that speaks; its card says so.
    let orpheus = identified(&folder("orpheus-3b", true));
    assert_withheld(&orpheus, Modality::speech());
    assert_eq!(orpheus.context_length, Some(131072));
    // CSM's architecture matches no rule; its card files it as speech even
    // though it also carries a text-to-audio tag.
    assert_withheld(&identified(&folder("csm-1b", true)), Modality::speech());
}

#[test]
fn outetts_is_a_voice_not_a_chat_model() {
    assert_withheld(&identified(&folder("outetts-1b", true)), Modality::speech());
}

#[test]
fn a_chat_model_tagged_music_keeps_chat() {
    // A Qwen3 fine-tune that writes music as code: filed as text generation,
    // tagged music, and shipping a chat template, so it is a chat model.
    let id = identified(&folder("decomposer-qwen3-8b", true));
    assert_eq!(id.modality, Some(Modality::text()));
    assert!(id.capabilities.contains(&Capability::chat()));
}

#[test]
fn a_bare_text_to_audio_card_only_keeps_the_model_out_of_chat() {
    // ChatTTS ships no config at all; its card says text-to-audio, and it is a
    // voice, so the card alone claims neither speech nor audio generation.
    let id = identified(&folder("chattts", false));
    assert_eq!(id.format, ModelFormat::Unknown);
    assert_withheld(&id, Modality::audio());
}

#[test]
fn no_fixture_both_speaks_and_makes_audio() {
    for entry in std::fs::read_dir(fixture("")).unwrap() {
        let entry = entry.unwrap();
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().into_string().unwrap();
        if name == "decomposer-qwen3-8b" {
            continue;
        }
        let id = identified(&folder(&name, true));
        assert!(
            !(id.capabilities.contains(&Capability::speak())
                && id.capabilities.contains(&Capability::audio())),
            "{name}"
        );
        assert!(!id.capabilities.contains(&Capability::chat()), "{name}");
    }
}

#[test]
fn a_plain_causal_lm_is_still_chat_with_or_without_a_card() {
    let dir = TempDir::new();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"architectures":["LlamaForCausalLM"],"model_type":"llama"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("model.safetensors"), b"weights").unwrap();
    assert!(identified(&dir).capabilities.contains(&Capability::chat()));
    std::fs::write(
        dir.path().join("README.md"),
        "---\npipeline_tag: text-generation\ntags:\n- llama\n---\n",
    )
    .unwrap();
    assert!(identified(&dir).capabilities.contains(&Capability::chat()));
}

#[test]
fn the_card_never_overrides_an_embedder() {
    let dir = TempDir::new();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"architectures":["BertModel"]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "---\npipeline_tag: text-to-audio\n---\n",
    )
    .unwrap();
    let hint = from_snapshot(dir.path()).expect("hint");
    assert_eq!(hint.modality, Some(Modality::embedding()));
}

#[test]
fn the_hf_scanner_hints_yue2_as_audio_not_chat() {
    let root = TempDir::new();
    let repo = root.path().join("models--m-a-p--YuE2-3B");
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs/main"), "abc").unwrap();
    let snapshot = repo.join("snapshots/abc");
    std::fs::create_dir_all(&snapshot).unwrap();
    for file in ["config.json", "README.md"] {
        std::fs::copy(fixture("yue2-3b").join(file), snapshot.join(file)).unwrap();
    }
    std::fs::write(snapshot.join("model.safetensors"), b"weights").unwrap();
    let result = HFCacheScanner::new(vec![root.path().to_path_buf()]).scan();
    let model = &result.discovered[0];
    assert_eq!(model.modality_hint, Some(Modality::audio()));
    assert!(model.capabilities_hint.is_empty());
    assert_eq!(model.execution_hint, ExecutionMode::Job);
}

#[test]
fn the_hf_scanner_reads_a_card_only_snapshot() {
    let root = TempDir::new();
    let repo = root.path().join("models--2Noise--ChatTTS");
    let snapshot = repo.join("snapshots/abc");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::create_dir_all(repo.join("refs")).unwrap();
    std::fs::write(repo.join("refs/main"), "abc").unwrap();
    std::fs::copy(fixture("chattts/README.md"), snapshot.join("README.md")).unwrap();
    let result = HFCacheScanner::new(vec![root.path().to_path_buf()]).scan();
    assert_eq!(result.discovered[0].modality_hint, Some(Modality::audio()));
}
