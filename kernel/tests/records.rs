//! Integration tests for the `records` data model: the string-newtype and enum
//! identifiers, the int/float-aware `JsonValue`, `ModelRecord` construction and
//! tolerant decoding, stable-id derivation, and the byte-format / text-budget
//! utilities. Public API only.

use std::collections::BTreeMap;

use kernel::records::{
    BidPreference, Capability, Clip, ExecutionMode, JsonValue, Modality, ModelRecord, ModelSource,
    ModelState, ParamSpec, ParamType, Resolution, RunTier, RuntimeId, RuntimeRef, SourceKind, clip,
    format_bytes, stable_id,
};

#[test]
fn string_newtypes_construct_and_expose_their_value() {
    assert_eq!(Capability::chat().as_str(), "chat");
    assert_eq!(Modality::text().as_str(), "text");
    assert_eq!(RuntimeId::llama_cpp().as_str(), "llama-cpp");
    assert_eq!(RuntimeId::mlx_lm().as_str(), "python:mlx-lm");
    assert_eq!(Capability::from("custom").as_str(), "custom");
    assert_eq!(Capability::from(String::from("owned")).as_str(), "owned");
    assert_eq!(format!("{}", Modality::vision()), "vision");
}

#[test]
fn string_newtypes_serialize_transparently() {
    assert_eq!(
        serde_json::to_string(&Capability::chat()).unwrap(),
        "\"chat\""
    );
    let parsed: Capability = serde_json::from_str("\"see\"").unwrap();
    assert_eq!(parsed, Capability::see());
}

#[test]
fn closed_enums_round_trip_with_their_wire_names() {
    assert_eq!(
        serde_json::to_string(&ExecutionMode::Sync).unwrap(),
        "\"sync\""
    );
    assert_eq!(
        serde_json::to_string(&RunTier::RecipeNeeded).unwrap(),
        "\"recipe-needed\""
    );
    assert_eq!(
        serde_json::to_string(&ModelState::Missing).unwrap(),
        "\"missing\""
    );
    assert_eq!(
        serde_json::to_string(&Resolution::User).unwrap(),
        "\"user\""
    );
    assert_eq!(serde_json::to_string(&ParamType::Enum).unwrap(), "\"enum\"");

    assert_eq!(
        serde_json::from_str::<RunTier>("\"recipe-needed\"").unwrap(),
        RunTier::RecipeNeeded
    );
    assert_eq!(
        serde_json::from_str::<ParamType>("\"enum\"").unwrap(),
        ParamType::Enum
    );
}

#[test]
fn enum_defaults_match_the_swift_model() {
    assert_eq!(ExecutionMode::default(), ExecutionMode::Sync);
    assert_eq!(RunTier::default(), RunTier::RecipeNeeded);
    assert_eq!(ModelState::default(), ModelState::Unresolved);
    assert_eq!(Resolution::default(), Resolution::Unresolved);
    assert_eq!(RuntimeRef::default().resolved, Resolution::Unresolved);
    assert_eq!(RuntimeRef::default().tier, RunTier::RecipeNeeded);
}

#[test]
fn bid_preferences_form_the_expected_ordering() {
    assert_eq!(BidPreference::LLAMA_CPP, 10);
    assert_eq!(BidPreference::MLX_VLM, 14);
    assert_eq!(BidPreference::OLLAMA, 20);
    assert_eq!(BidPreference::MANIFEST, 100);
}

#[test]
fn json_int_and_double_compare_equal_when_numerically_equal() {
    assert_eq!(JsonValue::Int(2), JsonValue::Double(2.0));
    assert_eq!(JsonValue::Double(2.0), JsonValue::Int(2));
    assert_ne!(JsonValue::Int(2), JsonValue::Double(2.5));
    assert_eq!(JsonValue::Int(2), JsonValue::Int(2));
    assert_ne!(JsonValue::Int(2), JsonValue::String("2".into()));
}

#[test]
fn json_cross_number_equality_is_recursive() {
    let a = JsonValue::Array(vec![JsonValue::Int(1), JsonValue::Int(2)]);
    let b = JsonValue::Array(vec![JsonValue::Double(1.0), JsonValue::Double(2.0)]);
    assert_eq!(a, b);

    let mut oa = BTreeMap::new();
    oa.insert("k".to_owned(), JsonValue::Int(3));
    let mut ob = BTreeMap::new();
    ob.insert("k".to_owned(), JsonValue::Double(3.0));
    assert_eq!(JsonValue::Object(oa), JsonValue::Object(ob));
}

#[test]
fn json_accessors_coerce_between_number_kinds() {
    assert_eq!(JsonValue::Double(4.9).as_i64(), Some(4));
    assert_eq!(JsonValue::Int(4).as_f64(), Some(4.0));
    assert_eq!(JsonValue::String("hi".into()).as_str(), Some("hi"));
    assert_eq!(JsonValue::Bool(true).as_bool(), Some(true));
    assert_eq!(JsonValue::Null.as_i64(), None);
    assert!(
        JsonValue::Array(vec![JsonValue::Int(1)])
            .as_array()
            .is_some()
    );
    assert!(JsonValue::Int(1).as_object().is_none());
}

#[test]
fn json_parsing_preserves_int_vs_float() {
    assert_eq!(
        serde_json::from_str::<JsonValue>("2").unwrap(),
        JsonValue::Int(2)
    );
    assert_eq!(
        serde_json::from_str::<JsonValue>("-7").unwrap(),
        JsonValue::Int(-7)
    );
    assert_eq!(
        serde_json::from_str::<JsonValue>("2.5").unwrap(),
        JsonValue::Double(2.5)
    );
    assert_eq!(
        serde_json::from_str::<JsonValue>("true").unwrap(),
        JsonValue::Bool(true)
    );
    assert_eq!(
        serde_json::from_str::<JsonValue>("null").unwrap(),
        JsonValue::Null
    );
    assert_eq!(
        serde_json::from_str::<JsonValue>("\"s\"").unwrap(),
        JsonValue::String("s".into())
    );
}

#[test]
fn json_huge_unsigned_falls_back_to_double() {
    let parsed: JsonValue = serde_json::from_str("18446744073709551615").unwrap();
    assert!(matches!(parsed, JsonValue::Double(_)));
}

#[test]
fn json_serializes_numbers_and_sorts_object_keys() {
    assert_eq!(serde_json::to_string(&JsonValue::Int(3)).unwrap(), "3");
    assert_eq!(
        serde_json::to_string(&JsonValue::Double(3.5)).unwrap(),
        "3.5"
    );
    assert_eq!(serde_json::to_string(&JsonValue::Null).unwrap(), "null");

    let mut fields = BTreeMap::new();
    fields.insert("b".to_owned(), JsonValue::Int(2));
    fields.insert("a".to_owned(), JsonValue::Int(1));
    assert_eq!(
        serde_json::to_string(&JsonValue::Object(fields)).unwrap(),
        r#"{"a":1,"b":2}"#
    );
}

#[test]
fn json_round_trips_through_serde() {
    let original: JsonValue =
        serde_json::from_str(r#"{"n":1,"f":1.5,"s":"x","b":true,"nil":null,"arr":[1,2,{"k":9}]}"#)
            .unwrap();
    let text = serde_json::to_string(&original).unwrap();
    let again: JsonValue = serde_json::from_str(&text).unwrap();
    assert_eq!(original, again);
}

#[test]
fn source_identity_is_kind_path_repo() {
    let mut source = ModelSource::new(SourceKind::file(), "/models/x.gguf");
    assert_eq!(source.identity(), "file|/models/x.gguf|");
    source.repo = Some("org/model".into());
    assert_eq!(source.identity(), "file|/models/x.gguf|org/model");
}

#[test]
fn stable_id_is_deterministic_hex_and_source_sensitive() {
    let source = ModelSource::new(SourceKind::file(), "/models/x.gguf");
    let id = stable_id(&source);
    assert_eq!(id.len(), 16);
    assert!(
        id.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    assert_eq!(id, stable_id(&source));

    let other = ModelSource::new(SourceKind::file(), "/models/y.gguf");
    assert_ne!(stable_id(&source), stable_id(&other));
}

#[test]
fn stable_id_depends_on_repo_but_not_ref() {
    let base = ModelSource::new(SourceKind::huggingface_cache(), "/hub/x");
    let mut with_repo = base.clone();
    with_repo.repo = Some("org/model".into());
    assert_ne!(stable_id(&base), stable_id(&with_repo));

    let mut with_ref = with_repo.clone();
    with_ref.reference = Some("main".into());
    assert_eq!(stable_id(&with_repo), stable_id(&with_ref));
}

fn a_record() -> ModelRecord {
    ModelRecord::new(
        "Test Model",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::file(), "/models/test.gguf"),
    )
}

#[test]
fn new_record_has_derived_id_and_sane_defaults() {
    let record = a_record();
    assert_eq!(record.id, stable_id(&record.source));
    assert_eq!(record.state, ModelState::Unresolved);
    assert_eq!(record.execution, ExecutionMode::Sync);
    assert!(!record.downloading);
    assert!(record.param_values.is_empty());
    assert!(record.registered_at > 0);
}

#[test]
fn display_name_prefers_a_non_empty_alias() {
    let mut record = a_record();
    assert_eq!(record.display_name(), "Test Model");
    record.alias = Some(String::new());
    assert_eq!(record.display_name(), "Test Model");
    record.alias = Some("Nickname".into());
    assert_eq!(record.display_name(), "Nickname");
}

#[test]
fn can_reports_capabilities() {
    let record = a_record();
    assert!(record.can(&Capability::chat()));
    assert!(!record.can(&Capability::embed()));
}

#[test]
fn record_round_trips_through_json() {
    let mut record = a_record();
    record
        .param_values
        .insert("temperature".into(), JsonValue::Double(0.7));
    record.params.push(ParamSpec {
        key: "temperature".into(),
        param_type: ParamType::Float,
        default_value: Some(JsonValue::Double(0.8)),
        range: Some(vec![JsonValue::Double(0.0), JsonValue::Double(2.0)]),
        values: None,
    });
    record.runtime = RuntimeRef {
        id: Some(RuntimeId::llama_cpp()),
        resolved: Resolution::Auto,
        tier: RunTier::Native,
        ..RuntimeRef::default()
    };
    let text = serde_json::to_string(&record).unwrap();
    let back: ModelRecord = serde_json::from_str(&text).unwrap();
    assert_eq!(record, back);
}

#[test]
fn record_decodes_from_minimal_json_with_defaults() {
    let json = r#"{
        "id": "abc123",
        "name": "Minimal",
        "modality": "text",
        "capabilities": ["chat"],
        "source": {"kind": "file", "path": "/m.gguf"},
        "registered_at": 1000
    }"#;
    let record: ModelRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.id, "abc123");
    assert_eq!(record.state, ModelState::Unresolved);
    assert_eq!(record.execution, ExecutionMode::Sync);
    assert!(!record.downloading);
    assert!(record.params.is_empty());
    assert_eq!(record.runtime, RuntimeRef::default());
}

#[test]
fn record_omits_empty_optionals_when_serialized() {
    let text = serde_json::to_string(&a_record()).unwrap();
    assert!(!text.contains("alias"));
    assert!(!text.contains("system_prompt"));
    assert!(!text.contains("downloading"));
    assert!(!text.contains("param_values"));
}

#[test]
fn record_equality_tracks_param_values() {
    let mut a = a_record();
    let mut b = a.clone();
    assert_eq!(a, b);
    a.param_values.insert("k".into(), JsonValue::Int(1));
    b.param_values.insert("k".into(), JsonValue::Double(1.0));
    assert_eq!(a, b, "int and float param values compare equal");
    b.param_values.insert("k".into(), JsonValue::Double(2.0));
    assert_ne!(a, b);
}

#[test]
fn byte_format_covers_each_unit() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1024), "1 KB");
    assert_eq!(format_bytes(1 << 20), "1 MB");
    assert_eq!(format_bytes(1 << 30), "1 GB");
    assert_eq!(format_bytes(2 << 30), "2 GB");
    assert_eq!(format_bytes(3 << 29), "1.5 GB");
}

#[test]
fn clip_keeps_short_text_untouched() {
    let result = clip("hello", 10);
    assert_eq!(
        result,
        Clip {
            kept: "hello",
            overflowed: false,
            total: 5
        }
    );
}

#[test]
fn clip_trims_ascii_to_the_cap() {
    let result = clip("hello world", 5);
    assert_eq!(result.kept, "hello");
    assert!(result.overflowed);
    assert_eq!(result.total, 11);
}

#[test]
fn clip_never_splits_a_multibyte_character() {
    let text = "héllo";
    assert_eq!(text.len(), 6);
    let result = clip(text, 2);
    assert!(result.overflowed);
    assert!(result.kept.len() <= 2);
    assert!(text.starts_with(result.kept));
    assert_eq!(result.kept, "h");
    assert_eq!(result.total, 6);
}

#[test]
fn clip_handles_zero_cap_and_exact_length() {
    assert_eq!(
        clip("", 0),
        Clip {
            kept: "",
            overflowed: false,
            total: 0
        }
    );
    let zero = clip("abc", 0);
    assert_eq!(zero.kept, "");
    assert!(zero.overflowed);
    let exact = clip("abc", 3);
    assert_eq!(
        exact,
        Clip {
            kept: "abc",
            overflowed: false,
            total: 3
        }
    );
}

#[test]
fn json_whole_number_float_stays_a_double_through_serde() {
    for value in [
        JsonValue::Double(3.0),
        JsonValue::Double(-2.0),
        JsonValue::Double(1e20),
    ] {
        let text = serde_json::to_string(&value).unwrap();
        let back: JsonValue = serde_json::from_str(&text).unwrap();
        assert!(
            matches!(back, JsonValue::Double(_)),
            "{text} did not stay a Double"
        );
    }
}

#[test]
fn json_as_i64_rejects_non_finite_and_out_of_range() {
    assert_eq!(JsonValue::Double(4.9).as_i64(), Some(4));
    assert_eq!(JsonValue::Double(-4.9).as_i64(), Some(-4));
    assert_eq!(JsonValue::Double(f64::NAN).as_i64(), None);
    assert_eq!(JsonValue::Double(f64::INFINITY).as_i64(), None);
    assert_eq!(JsonValue::Double(-f64::INFINITY).as_i64(), None);
    assert_eq!(JsonValue::Double(1e30).as_i64(), None);
    assert_eq!(JsonValue::Double(-1e30).as_i64(), None);
}

#[test]
fn json_non_finite_double_serializes_to_null() {
    assert_eq!(
        serde_json::to_string(&JsonValue::Double(f64::NAN)).unwrap(),
        "null"
    );
    assert_eq!(
        serde_json::to_string(&JsonValue::Double(f64::INFINITY)).unwrap(),
        "null"
    );
}

#[test]
fn json_deep_nesting_round_trips() {
    let source = r#"{"a":[1,{"b":[2.5,{"c":[null,true,"x"]}]}],"d":{"e":{"f":-9}}}"#;
    let value: JsonValue = serde_json::from_str(source).unwrap();
    let text = serde_json::to_string(&value).unwrap();
    let again: JsonValue = serde_json::from_str(&text).unwrap();
    assert_eq!(value, again);
    let deep = value.as_object().unwrap()["a"].as_array().unwrap()[1]
        .as_object()
        .unwrap()["b"]
        .as_array()
        .unwrap()[0]
        .as_f64();
    assert_eq!(deep, Some(2.5));
}

#[test]
fn json_from_conversions_build_values() {
    assert_eq!(JsonValue::from(3i64), JsonValue::Int(3));
    assert_eq!(JsonValue::from(3.5f64), JsonValue::Double(3.5));
    assert_eq!(JsonValue::from(true), JsonValue::Bool(true));
    assert_eq!(JsonValue::from("s"), JsonValue::String("s".into()));
    assert_eq!(JsonValue::default(), JsonValue::Null);
}

#[test]
fn json_large_int_equality_is_not_transitive_by_design() {
    let big = JsonValue::Int(9_007_199_254_740_993); // 2^53 + 1, not representable as f64
    let smaller = JsonValue::Int(9_007_199_254_740_992); // 2^53
    let as_double = JsonValue::Double(9_007_199_254_740_992.0);
    assert_eq!(big, as_double);
    assert_eq!(smaller, as_double);
    assert_ne!(big, smaller);
}

#[test]
fn stable_id_is_injective_across_field_boundaries() {
    let mut a = ModelSource::new(SourceKind::file(), "a|b");
    a.repo = Some("c".into());
    let mut b = ModelSource::new(SourceKind::file(), "a");
    b.repo = Some("b|c".into());
    assert_ne!(
        stable_id(&a),
        stable_id(&b),
        "pipe in a field must not cause a collision"
    );
}

#[test]
fn byte_format_handles_boundaries_and_negatives() {
    assert_eq!(format_bytes((1 << 20) - 1), "1023 KB");
    assert_eq!(format_bytes((1 << 10) - 1), "1023 B");
    assert_eq!(format_bytes((1 << 30) - 1), "1023 MB");
    assert_eq!(format_bytes(-5), "-5 B");
}
