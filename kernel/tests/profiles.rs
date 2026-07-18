//! Integration tests for the `profiles` unit: context budgeting, configuration
//! merging (param overrides + system-prompt seeding), and the built-in parameter
//! schema assembly. Public API only.

mod support;

use kernel::profiles::{
    ProfileRegistry, Verdict, assess, context_length_spec, dropping_vanished_param_values,
    effective_window, estimated_tokens, merged, normalized_param_values, prompt_characters,
    stored_context_length,
};
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, ParamSpec, ParamType, RuntimeId,
    SourceKind,
};

fn chat_record_on(runtime: RuntimeId) -> ModelRecord {
    let mut record = ModelRecord::new(
        "Model",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/m.gguf"),
    );
    record.runtime.id = Some(runtime);
    record
}

fn json(text: &str) -> JsonValue {
    serde_json::from_str(text).unwrap()
}

fn keys(specs: &[ParamSpec]) -> Vec<&str> {
    specs.iter().map(|spec| spec.key.as_str()).collect()
}

#[test]
fn estimated_tokens_is_four_characters_each() {
    assert_eq!(estimated_tokens(0), 0);
    assert_eq!(estimated_tokens(1), 1);
    assert_eq!(estimated_tokens(4), 1);
    assert_eq!(estimated_tokens(5), 2);
    assert_eq!(estimated_tokens(9), 3);
}

#[test]
fn assess_fits_clamps_to_remaining_window() {
    let window = 10_000;
    let prompt_chars = 1_000;
    assert_eq!(
        assess(prompt_chars, window, None),
        Verdict::Fits {
            clamped_max_tokens: Some(9_750)
        }
    );
    assert_eq!(
        assess(prompt_chars, window, Some(100)),
        Verdict::Fits {
            clamped_max_tokens: Some(100)
        }
    );
    assert_eq!(
        assess(prompt_chars, window, Some(999_999)),
        Verdict::Fits {
            clamped_max_tokens: Some(9_750)
        }
    );
}

#[test]
fn assess_exceeds_when_prompt_plus_floor_overflows() {
    assert_eq!(
        assess(1_000, 300, None),
        Verdict::Exceeds {
            estimated: 250,
            window: 300
        }
    );
}

#[test]
fn effective_window_follows_per_runtime_policy() {
    let mut builtin = chat_record_on(RuntimeId::llama_cpp());
    builtin.source = ModelSource::new(SourceKind::builtin(), "apple");
    assert_eq!(effective_window(&builtin, None), Some(4096));

    let mut llama = chat_record_on(RuntimeId::llama_cpp());
    llama.context_length = Some(8192);
    assert_eq!(effective_window(&llama, None), Some(8192));

    let mut ollama = chat_record_on(RuntimeId::ollama());
    ollama.context_length = Some(4096);
    assert_eq!(effective_window(&ollama, Some(2048)), Some(2048));
    assert_eq!(effective_window(&ollama, None), Some(4096));

    let unknown = chat_record_on(RuntimeId::from("mystery"));
    assert_eq!(effective_window(&unknown, None), None);

    let mut zero = chat_record_on(RuntimeId::llama_cpp());
    zero.context_length = Some(0);
    assert_eq!(effective_window(&zero, None), None);
}

#[test]
fn prompt_characters_sums_message_content_and_prompt() {
    let payload = json(
        r#"{"messages":[{"role":"user","content":"hello"},{"content":"world!"}],"prompt":"x"}"#,
    );
    assert_eq!(prompt_characters(&payload), 12);
    assert_eq!(prompt_characters(&json("[]")), 0);
    assert_eq!(prompt_characters(&json(r#"{"other":1}"#)), 0);
}

#[test]
fn normalized_param_values_drops_vanished_keys() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "temperature".into(),
        param_type: ParamType::Float,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("temperature".into(), JsonValue::Double(0.5));
    record
        .param_values
        .insert("ghost".into(), JsonValue::Int(1));

    let normalized = normalized_param_values(&record);
    assert_eq!(normalized.len(), 1);
    assert!(normalized.contains_key("temperature"));

    let scrubbed = dropping_vanished_param_values(&record);
    assert_eq!(scrubbed.param_values.len(), 1);
}

#[test]
fn normalized_param_values_drops_a_wrong_typed_value() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "mode".into(),
        param_type: ParamType::Enum,
        default_value: None,
        range: None,
        values: Some(vec!["fast".into(), "slow".into()]),
    });
    // A spec'd key whose saved value can't be normalized (an int for an enum, an
    // off-list string) is dropped, not passed through to the runtime.
    record.param_values.insert("mode".into(), JsonValue::Int(1));

    assert!(!normalized_param_values(&record).contains_key("mode"));
}

#[test]
fn stored_context_length_reads_a_recognized_value() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "context_length".into(),
        param_type: ParamType::Int,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("context_length".into(), JsonValue::Int(4096));
    assert_eq!(stored_context_length(&record), Some(4096));

    record.params.clear();
    assert_eq!(
        stored_context_length(&record),
        None,
        "value without a spec is dropped"
    );
}

#[test]
fn merged_seeds_a_system_prompt_for_chat() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("be terse".into());
    let payload = json(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
    let result = merged(&record, &Capability::chat(), payload, None, None, None);

    let messages = result.as_object().unwrap()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    let first = messages[0].as_object().unwrap();
    assert_eq!(first["role"].as_str(), Some("system"));
    assert_eq!(first["content"].as_str(), Some("be terse"));
}

#[test]
fn merged_prefers_session_then_record_then_fallback_prompt() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("record".into());
    let payload = json(r#"{"messages":[{"role":"user","content":"hi"}]}"#);

    let session = merged(
        &record,
        &Capability::chat(),
        payload.clone(),
        Some("fallback"),
        Some("session"),
        None,
    );
    assert_eq!(system_content(&session), "session");

    let from_record = merged(
        &record,
        &Capability::chat(),
        payload.clone(),
        Some("fallback"),
        None,
        None,
    );
    assert_eq!(system_content(&from_record), "record");

    let mut no_prompt = record.clone();
    no_prompt.system_prompt = None;
    let fallback = merged(
        &no_prompt,
        &Capability::chat(),
        payload,
        Some("fallback"),
        None,
        None,
    );
    assert_eq!(system_content(&fallback), "fallback");
}

#[test]
fn merged_appends_block_to_existing_system_message() {
    let record = chat_record_on(RuntimeId::llama_cpp());
    let payload =
        json(r#"{"messages":[{"role":"system","content":"base"},{"role":"user","content":"hi"}]}"#);
    let result = merged(
        &record,
        &Capability::chat(),
        payload,
        None,
        None,
        Some("extra"),
    );
    assert_eq!(system_content(&result), "base\n\nextra");
}

#[test]
fn merged_fills_only_absent_param_keys() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "temperature".into(),
        param_type: ParamType::Float,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("temperature".into(), JsonValue::Double(0.2));
    let payload = json(r#"{"temperature":0.9,"messages":[]}"#);
    let result = merged(&record, &Capability::chat(), payload, None, None, None);
    assert_eq!(
        result.as_object().unwrap()["temperature"],
        JsonValue::Double(0.9)
    );
}

#[test]
fn merged_returns_payload_unchanged_when_nothing_to_do() {
    let record = chat_record_on(RuntimeId::llama_cpp());
    let payload = json(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
    let result = merged(
        &record,
        &Capability::chat(),
        payload.clone(),
        None,
        None,
        None,
    );
    assert_eq!(result, payload);

    let scalar = JsonValue::Int(7);
    assert_eq!(
        merged(
            &record,
            &Capability::chat(),
            scalar.clone(),
            None,
            None,
            None
        ),
        scalar
    );
}

#[test]
fn merged_ignores_prompt_for_non_chat_but_applies_overrides() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("ignored".into());
    record.params.push(ParamSpec {
        key: "speed".into(),
        param_type: ParamType::Float,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("speed".into(), JsonValue::Double(1.5));
    let payload = json(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
    let result = merged(&record, &Capability::speak(), payload, None, None, None);
    let object = result.as_object().unwrap();
    assert_eq!(object["speed"], JsonValue::Double(1.5));
    assert_eq!(
        object["messages"].as_array().unwrap().len(),
        1,
        "no system prompt seeded"
    );
}

#[test]
fn builtin_schema_covers_text_sampling_and_context() {
    let registry = ProfileRegistry::builtin();
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.context_length = Some(8192);
    let schema = registry.schema(&record);
    let names = keys(&schema);
    for expected in [
        "temperature",
        "top_p",
        "max_tokens",
        "top_k",
        "min_p",
        "context_length",
    ] {
        assert!(names.contains(&expected), "missing {expected} in {names:?}");
    }
    assert_eq!(
        schema
            .iter()
            .filter(|spec| spec.key == "temperature")
            .count(),
        1,
        "duplicate keys must be deduped"
    );
}

#[test]
fn builtin_schema_matches_speech_and_transcription() {
    let registry = ProfileRegistry::builtin();
    let mut speaker = ModelRecord::new(
        "Voice",
        Modality::speech(),
        vec![Capability::speak()],
        ModelSource::new(SourceKind::file(), "/v"),
    );
    speaker.runtime.id = Some(RuntimeId::mlx_audio());
    assert_eq!(keys(&registry.schema(&speaker)), ["voice", "speed"]);

    let mut scribe = ModelRecord::new(
        "Whisper",
        Modality::audio(),
        vec![Capability::transcribe()],
        ModelSource::new(SourceKind::file(), "/w"),
    );
    scribe.runtime.id = Some(RuntimeId::whisper_cpp());
    assert_eq!(keys(&registry.schema(&scribe)), ["language", "translate"]);
}

#[test]
fn thinking_toggle_only_for_thinking_runtimes() {
    let registry = ProfileRegistry::builtin();
    let ollama = chat_record_on(RuntimeId::ollama());
    assert!(keys(&registry.schema(&ollama)).contains(&"thinking"));
    let llama = chat_record_on(RuntimeId::llama_cpp());
    assert!(!keys(&registry.schema(&llama)).contains(&"thinking"));
}

#[test]
fn context_length_spec_sizes_to_the_window() {
    let mut sized = chat_record_on(RuntimeId::ollama());
    sized.context_length = Some(8192);
    let spec = context_length_spec(&sized).unwrap();
    assert_eq!(spec.default_value, Some(JsonValue::Int(8192)));
    assert_eq!(
        spec.range,
        Some(vec![JsonValue::Int(512), JsonValue::Int(8192)])
    );

    let open_ended = chat_record_on(RuntimeId::ollama());
    let open = context_length_spec(&open_ended).unwrap();
    assert_eq!(open.default_value, None);
    assert_eq!(
        open.range,
        Some(vec![JsonValue::Int(512), JsonValue::Int(131072)])
    );

    let unsupported = chat_record_on(RuntimeId::mlx_swift());
    assert!(context_length_spec(&unsupported).is_none());
}

#[test]
fn refreshed_writes_the_schema_onto_the_record() {
    let registry = ProfileRegistry::builtin();
    let record = chat_record_on(RuntimeId::llama_cpp());
    assert!(record.params.is_empty());
    let refreshed = registry.refreshed(&record);
    assert!(!refreshed.params.is_empty());
}

#[test]
fn assess_boundary_between_fit_and_exceed() {
    assert_eq!(
        assess(1_000, 506, None),
        Verdict::Fits {
            clamped_max_tokens: Some(256)
        }
    );
    assert_eq!(
        assess(1_000, 505, None),
        Verdict::Exceeds {
            estimated: 250,
            window: 505
        }
    );
}

#[test]
fn assess_is_total_over_non_positive_windows() {
    assert_eq!(
        assess(0, 0, None),
        Verdict::Exceeds {
            estimated: 0,
            window: 0
        }
    );
    assert_eq!(
        assess(0, -5, None),
        Verdict::Exceeds {
            estimated: 0,
            window: -5
        }
    );
}

#[test]
fn estimated_tokens_truncates_toward_zero_for_negatives() {
    assert_eq!(estimated_tokens(-1), 0);
    assert_eq!(estimated_tokens(-10), -1);
}

#[test]
fn effective_window_rejects_non_positive_ollama_override() {
    let ollama = chat_record_on(RuntimeId::ollama());
    assert_eq!(effective_window(&ollama, Some(0)), None);
}

#[test]
fn stored_context_length_accepts_an_integral_double() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "context_length".into(),
        param_type: ParamType::Int,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("context_length".into(), JsonValue::Double(4096.0));
    assert_eq!(stored_context_length(&record), Some(4096));
}

#[test]
fn merged_with_prompt_but_no_messages_key_applies_overrides_only() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("hi".into());
    record.params.push(ParamSpec {
        key: "temperature".into(),
        param_type: ParamType::Float,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("temperature".into(), JsonValue::Double(0.3));
    let payload = json(r#"{"other":1}"#);
    let result = merged(&record, &Capability::chat(), payload, None, None, None);
    let object = result.as_object().unwrap();
    assert_eq!(object["temperature"], JsonValue::Double(0.3));
    assert!(
        !object.contains_key("messages"),
        "no messages key means no seeding"
    );
}

#[test]
fn merged_leaves_non_array_messages_untouched() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("hi".into());
    let payload = json(r#"{"messages":"oops"}"#);
    let result = merged(
        &record,
        &Capability::chat(),
        payload.clone(),
        None,
        None,
        None,
    );
    assert_eq!(result, payload);
}

#[test]
fn merged_null_payload_with_overrides_becomes_an_object() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.params.push(ParamSpec {
        key: "seed".into(),
        param_type: ParamType::Int,
        default_value: None,
        range: None,
        values: None,
    });
    record
        .param_values
        .insert("seed".into(), JsonValue::Int(42));
    let result = merged(
        &record,
        &Capability::chat(),
        JsonValue::Null,
        None,
        None,
        None,
    );
    assert_eq!(result.as_object().unwrap()["seed"], JsonValue::Int(42));
}

#[test]
fn merged_block_only_appends_and_prompt_only_prepends_joined() {
    let record = chat_record_on(RuntimeId::llama_cpp());
    let payload = json(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
    let result = merged(
        &record,
        &Capability::chat(),
        payload,
        None,
        Some("prompt"),
        Some("block"),
    );
    assert_eq!(system_content(&result), "prompt\n\nblock");
}

#[test]
fn merged_drops_prompt_when_a_system_turn_exists_and_no_block() {
    let mut record = chat_record_on(RuntimeId::llama_cpp());
    record.system_prompt = Some("new-prompt".into());
    let payload =
        json(r#"{"messages":[{"role":"system","content":"base"},{"role":"user","content":"hi"}]}"#);
    let result = merged(&record, &Capability::chat(), payload, None, None, None);
    assert_eq!(
        system_content(&result),
        "base",
        "existing system turn is left as-is"
    );
}

fn system_content(payload: &JsonValue) -> String {
    payload.as_object().unwrap()["messages"].as_array().unwrap()[0]
        .as_object()
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned()
}
