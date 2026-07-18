//! Tests for the identity/bid foundation types.

use std::collections::HashSet;

use kernel::records::{Capability, ExecutionMode, Modality, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};

#[test]
fn identified_model_new_defaults_the_extra_fields() {
    let model = IdentifiedModel::new(
        ModelFormat::Gguf,
        Some(Modality::text()),
        vec![Capability::chat(), Capability::complete()],
        ExecutionMode::Stream,
    );
    assert_eq!(model.format, ModelFormat::Gguf);
    assert_eq!(model.modality, Some(Modality::text()));
    assert_eq!(
        model.capabilities,
        vec![Capability::chat(), Capability::complete()]
    );
    assert_eq!(model.execution, ExecutionMode::Stream);
    assert!(model.params.is_empty());
    assert_eq!(model.pipeline_class, None);
    assert_eq!(model.context_length, None);
    assert_eq!(model.has_chat_template, None);
}

#[test]
fn identified_model_carries_optional_facts() {
    let mut model =
        IdentifiedModel::new(ModelFormat::Diffusers, None, Vec::new(), ExecutionMode::Job);
    model.pipeline_class = Some("StableDiffusionPipeline".to_owned());
    model.context_length = Some(4096);
    model.has_chat_template = Some(true);
    assert_eq!(
        model.pipeline_class.as_deref(),
        Some("StableDiffusionPipeline")
    );
    assert_eq!(model.context_length, Some(4096));
    assert_eq!(model.has_chat_template, Some(true));
}

#[test]
fn runtime_bid_new_has_no_alternatives() {
    let bid = RuntimeBid::new(RunTier::Native, 10);
    assert_eq!(bid.tier, RunTier::Native);
    assert_eq!(bid.preference, 10);
    assert!(bid.alternatives.is_empty());
}

#[test]
fn runtime_bid_carries_alternatives_and_is_hashable() {
    let bid = RuntimeBid::with_alternatives(
        RunTier::Managed,
        5,
        vec![RuntimeId::ollama(), RuntimeId::llama_cpp()],
    );
    assert_eq!(bid.alternatives.len(), 2);

    // Eq + Hash: a bid can live in a set.
    let mut set = HashSet::new();
    set.insert(bid.clone());
    assert!(set.contains(&bid));
    assert!(!set.insert(bid), "an equal bid is not inserted twice");
}

#[test]
fn model_format_round_trips_the_logical_variants() {
    for (format, wire) in [
        (ModelFormat::Gguf, "\"gguf\""),
        (ModelFormat::GgmlBin, "\"ggml-bin\""),
        (ModelFormat::MlxSafetensors, "\"mlx-safetensors\""),
        (ModelFormat::Diffusers, "\"diffusers\""),
        (ModelFormat::OllamaStore, "\"ollama-store\""),
        (ModelFormat::Builtin, "\"builtin\""),
        (ModelFormat::Endpoint, "\"endpoint\""),
        (ModelFormat::Unknown, "\"unknown\""),
    ] {
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, wire, "{format:?}");
        let back: ModelFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(back, format);
    }
}
