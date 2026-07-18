//! Tests for `config.json` modality hinting.

mod support;

use kernel::discovery::modality_hints::{from_config, from_config_json, from_model_index};
use kernel::records::{Capability, ExecutionMode, JsonValue, Modality};
use support::TempDir;

fn object<const N: usize>(pairs: [(&str, JsonValue); N]) -> JsonValue {
    JsonValue::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn archs(names: &[&str]) -> JsonValue {
    JsonValue::Array(
        names
            .iter()
            .map(|n| JsonValue::String((*n).to_owned()))
            .collect(),
    )
}

#[test]
fn a_causal_lm_is_a_text_model() {
    let config = object([
        ("architectures", archs(&["LlamaForCausalLM"])),
        ("max_position_embeddings", JsonValue::Int(8192)),
    ]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::text()));
    assert!(hint.capabilities.contains(&Capability::chat()));
    assert!(hint.capabilities.contains(&Capability::complete()));
    assert_eq!(hint.execution, ExecutionMode::Stream);
    assert_eq!(hint.context_length, Some(8192));
}

#[test]
fn an_lm_head_substring_is_text() {
    let config = object([("architectures", archs(&["GPTBigCodeLMHeadModel"]))]);
    assert_eq!(
        from_config(&config).unwrap().modality,
        Some(Modality::text())
    );
}

#[test]
fn a_kokoro_architecture_is_speech() {
    let config = object([("architectures", archs(&["KokoroModel"]))]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::speech()));
    assert_eq!(hint.capabilities, vec![Capability::speak()]);
}

#[test]
fn a_whisper_architecture_is_audio() {
    let config = object([("architectures", archs(&["WhisperForConditionalGeneration"]))]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::audio()));
    assert_eq!(hint.capabilities, vec![Capability::transcribe()]);
}

#[test]
fn a_bert_architecture_is_an_embedding_model() {
    let config = object([("architectures", archs(&["BertModel"]))]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::embedding()));
    assert_eq!(hint.capabilities, vec![Capability::embed()]);
}

#[test]
fn a_vision_config_plus_matching_arch_is_vision_chat() {
    let config = object([
        ("architectures", archs(&["LlavaForConditionalGeneration"])),
        (
            "vision_config",
            object([("hidden_size", JsonValue::Int(1024))]),
        ),
        ("n_positions", JsonValue::Int(4096)),
    ]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::text()));
    assert!(hint.capabilities.contains(&Capability::see()));
    assert!(hint.capabilities.contains(&Capability::chat()));
    assert_eq!(hint.context_length, Some(4096));
}

#[test]
fn vision_language_matches_by_substring_not_only_the_suffix() {
    // Qwen2VL does not end in `ForConditionalGeneration`; the vision-language
    // substring list must still recognize it (with a vision_config present).
    let config = object([
        ("architectures", archs(&["Qwen2VLModel"])),
        ("vision_config", object([])),
    ]);
    let hint = from_config(&config).expect("hint");
    assert!(hint.capabilities.contains(&Capability::see()));
    assert!(hint.capabilities.contains(&Capability::chat()));
}

#[test]
fn a_vision_config_without_a_matching_arch_falls_through() {
    // vision_config alone is not enough — the architecture must also match.
    let config = object([
        ("architectures", archs(&["BertModel"])),
        ("vision_config", object([])),
    ]);
    let hint = from_config(&config).expect("hint");
    assert_eq!(hint.modality, Some(Modality::embedding()));
    assert!(!hint.capabilities.contains(&Capability::see()));
}

#[test]
fn speech_config_keys_are_recognized_without_an_architecture() {
    let istft = object([("istftnet", JsonValue::Bool(true))]);
    assert_eq!(
        from_config(&istft).unwrap().modality,
        Some(Modality::speech())
    );

    let plbert = object([("plbert", object([]))]);
    assert_eq!(
        from_config(&plbert).unwrap().modality,
        Some(Modality::speech())
    );

    // style_dim AND n_mels together.
    let styletts = object([
        ("style_dim", JsonValue::Int(128)),
        ("n_mels", JsonValue::Int(80)),
    ]);
    assert_eq!(
        from_config(&styletts).unwrap().modality,
        Some(Modality::speech())
    );
    // style_dim alone is not enough.
    let partial = object([("style_dim", JsonValue::Int(128))]);
    assert_eq!(from_config(&partial), None);
}

#[test]
fn context_length_prefers_the_first_present_window_key() {
    let n_positions = object([
        ("architectures", archs(&["GPT2ForCausalLM"])),
        ("n_positions", JsonValue::Int(1024)),
    ]);
    assert_eq!(
        from_config(&n_positions).unwrap().context_length,
        Some(1024)
    );

    let max_seq_len = object([
        ("architectures", archs(&["FooForCausalLM"])),
        ("max_seq_len", JsonValue::Int(2048)),
    ]);
    assert_eq!(
        from_config(&max_seq_len).unwrap().context_length,
        Some(2048)
    );
}

#[test]
fn a_present_but_zero_window_voids_the_context_and_does_not_fall_through() {
    let config = object([
        ("architectures", archs(&["FooForCausalLM"])),
        ("max_position_embeddings", JsonValue::Int(0)),
        ("n_positions", JsonValue::Int(4096)),
    ]);
    // Swift short-circuits on the first present key (0), then the >0 filter voids
    // it — it must NOT fall through to n_positions.
    assert_eq!(from_config(&config).unwrap().context_length, None);
}

#[test]
fn a_match_without_window_keys_has_no_context() {
    let config = object([("architectures", archs(&["LlamaForCausalLM"]))]);
    assert_eq!(from_config(&config).unwrap().context_length, None);
}

#[test]
fn an_unrecognized_config_is_none() {
    let config = object([("architectures", archs(&["MysteryNet"]))]);
    assert_eq!(from_config(&config), None);
    // A non-object is also None.
    assert_eq!(from_config(&JsonValue::Array(vec![])), None);
}

#[test]
fn from_config_json_reads_and_parses_a_file() {
    let dir = TempDir::new();
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        br#"{"architectures":["MistralForCausalLM"],"max_position_embeddings":32768}"#,
    )
    .unwrap();
    let hint = from_config_json(&path).expect("hint");
    assert_eq!(hint.modality, Some(Modality::text()));
    assert_eq!(hint.context_length, Some(32768));

    // A missing file is None, not a panic.
    assert_eq!(from_config_json(&dir.join("nope.json")), None);
    // Malformed JSON is None.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, b"{ not json").unwrap();
    assert_eq!(from_config_json(&bad), None);
}

#[test]
fn from_model_index_maps_a_known_class_to_an_image_job() {
    let dir = TempDir::new();
    let path = dir.join("model_index.json");
    std::fs::write(&path, br#"{"_class_name":"FluxPipeline"}"#).unwrap();
    let hint = from_model_index(&path);
    assert_eq!(hint.modality, Some(Modality::image()));
    assert!(hint.capabilities.contains(&Capability::image()));
    assert_eq!(hint.execution, ExecutionMode::Job);
}

#[test]
fn from_model_index_maps_a_video_class_to_a_video_job() {
    let dir = TempDir::new();
    let path = dir.join("model_index.json");
    std::fs::write(&path, br#"{"_class_name":"CogVideoXPipeline"}"#).unwrap();
    let hint = from_model_index(&path);
    assert_eq!(hint.modality, Some(Modality::video()));
    // Video/edit families carry no capabilities, only a modality.
    assert!(hint.capabilities.is_empty());
    assert_eq!(hint.execution, ExecutionMode::Job);
}

#[test]
fn from_model_index_is_a_bare_job_for_an_unknown_or_absent_class() {
    let dir = TempDir::new();
    // Unknown class → empty job hint (still a job).
    let unknown = dir.join("model_index.json");
    std::fs::write(&unknown, br#"{"_class_name":"MysteryPipeline"}"#).unwrap();
    let hint = from_model_index(&unknown);
    assert_eq!(hint.modality, None);
    assert!(hint.capabilities.is_empty());
    assert_eq!(hint.execution, ExecutionMode::Job);

    // A missing/unparseable file is also a bare job, not a panic.
    let hint = from_model_index(&dir.join("nope.json"));
    assert_eq!(hint.modality, None);
    assert_eq!(hint.execution, ExecutionMode::Job);
}
