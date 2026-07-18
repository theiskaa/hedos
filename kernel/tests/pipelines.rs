//! Tests for the diffusers `PipelineFamilyRegistry`: class-name lookup, the
//! scheduler-refinement param overrides, and the `matches`/`name_tokens` logic.

use std::collections::HashSet;

use kernel::records::{Capability, JsonValue, Modality, ParamSpec, ParamType};
use kernel::resolution::pipelines::{
    PipelineFamily, PipelineFamilyRegistry, PipelineRefinement, SchedulerFacts,
};

fn class_set(names: &[&str]) -> HashSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn spec(key: &str, default: i64) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Int,
        default_value: Some(JsonValue::Int(default)),
        range: None,
        values: None,
    }
}

#[test]
fn a_known_class_resolves_to_its_family_profile() {
    let registry = PipelineFamilyRegistry::builtin();
    let profile = registry
        .profile("StableDiffusionXLPipeline", None, None)
        .expect("sdxl profile");
    assert_eq!(profile.modality, Modality::image());
    assert!(profile.capabilities.contains(&Capability::image()));
    // The base SDXL preset: 30 steps.
    let steps = profile
        .params
        .iter()
        .find(|spec| spec.key == "steps")
        .expect("steps param");
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(30))
    );
}

#[test]
fn an_unknown_class_has_no_profile() {
    let registry = PipelineFamilyRegistry::builtin();
    assert!(registry.profile("NopePipeline", None, None).is_none());
    assert!(registry.family("NopePipeline").is_none());
}

#[test]
fn edit_and_video_and_audio_families_carry_only_a_modality() {
    let registry = PipelineFamilyRegistry::builtin();
    let edit = registry
        .profile("StableDiffusionImg2ImgPipeline", None, None)
        .expect("edit profile");
    assert_eq!(edit.modality, Modality::image());
    assert!(edit.capabilities.is_empty());
    assert!(edit.params.is_empty());

    let video = registry
        .profile("CogVideoXPipeline", None, None)
        .expect("video profile");
    assert_eq!(video.modality, Modality::video());

    let audio = registry
        .profile("StableAudioPipeline", None, None)
        .expect("audio profile");
    assert_eq!(audio.modality, Modality::audio());
}

#[test]
fn a_turbo_scheduler_and_name_signal_override_the_step_and_guidance_params() {
    let registry = PipelineFamilyRegistry::builtin();
    let scheduler = SchedulerFacts::new(
        Some("EulerAncestralDiscreteScheduler".to_owned()),
        Some("trailing".to_owned()),
    );
    let profile = registry
        .profile(
            "StableDiffusionXLPipeline",
            Some(&scheduler),
            Some("sdxl-turbo"),
        )
        .expect("turbo profile");
    let steps = profile.params.iter().find(|s| s.key == "steps").unwrap();
    let guidance = profile.params.iter().find(|s| s.key == "guidance").unwrap();
    // Turbo overrides: 2 steps, zero guidance.
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(2))
    );
    assert_eq!(
        guidance.default_value,
        Some(kernel::records::JsonValue::Double(0.0))
    );
    // Overrides replace by key — the schema length is unchanged.
    assert_eq!(profile.params.len(), 5);
}

#[test]
fn the_refinement_needs_both_the_scheduler_and_a_name_signal() {
    let registry = PipelineFamilyRegistry::builtin();
    let scheduler = SchedulerFacts::new(
        Some("EulerAncestralDiscreteScheduler".to_owned()),
        Some("trailing".to_owned()),
    );
    // Right scheduler, but the repo name carries no turbo/lightning/lcm token.
    let profile = registry
        .profile(
            "StableDiffusionXLPipeline",
            Some(&scheduler),
            Some("sdxl-base"),
        )
        .expect("base profile");
    let steps = profile.params.iter().find(|s| s.key == "steps").unwrap();
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(30))
    );
}

#[test]
fn the_wrong_timestep_spacing_does_not_refine() {
    let registry = PipelineFamilyRegistry::builtin();
    // Correct scheduler class and a turbo name, but leading (not trailing) spacing.
    let scheduler = SchedulerFacts::new(
        Some("EulerAncestralDiscreteScheduler".to_owned()),
        Some("leading".to_owned()),
    );
    let profile = registry
        .profile(
            "StableDiffusionPipeline",
            Some(&scheduler),
            Some("sd-turbo"),
        )
        .expect("profile");
    let steps = profile.params.iter().find(|s| s.key == "steps").unwrap();
    assert_eq!(
        steps.default_value,
        Some(kernel::records::JsonValue::Int(30))
    );
}

#[test]
fn a_refinement_can_append_a_new_param_key() {
    // A hand-built family whose refinement introduces a key absent from the base
    // schema — exercising the append (not replace) branch of `profile`.
    let refinement = PipelineRefinement {
        scheduler_classes: class_set(&["CustomScheduler"]),
        timestep_spacing: None,
        name_signals: HashSet::new(),
        param_overrides: vec![spec("steps", 2), spec("shift", 3)],
    };
    let family = PipelineFamily {
        id: "custom".to_owned(),
        class_names: class_set(&["CustomPipeline"]),
        modality: Modality::image(),
        capabilities: vec![Capability::image()],
        params: vec![spec("steps", 20)],
        refinements: vec![refinement],
    };
    let registry = PipelineFamilyRegistry::new(vec![family]);
    let scheduler = SchedulerFacts::new(Some("CustomScheduler".to_owned()), None);

    let profile = registry
        .profile("CustomPipeline", Some(&scheduler), None)
        .expect("profile");
    // `steps` replaced in place; `shift` appended.
    assert_eq!(profile.params.len(), 2);
    let steps = profile.params.iter().find(|s| s.key == "steps").unwrap();
    assert_eq!(steps.default_value, Some(JsonValue::Int(2)));
    assert!(profile.params.iter().any(|s| s.key == "shift"));
}

#[test]
fn an_empty_name_signal_refinement_matches_on_the_scheduler_alone() {
    // No name signals → the scheduler (and spacing) alone decide, even with no
    // repo hint.
    let refinement = PipelineRefinement {
        scheduler_classes: class_set(&["CustomScheduler"]),
        timestep_spacing: None,
        name_signals: HashSet::new(),
        param_overrides: vec![spec("steps", 1)],
    };
    let scheduler = SchedulerFacts::new(Some("CustomScheduler".to_owned()), None);
    assert!(refinement.matches(&scheduler, None));
    // A different scheduler class does not match.
    let other = SchedulerFacts::new(Some("OtherScheduler".to_owned()), None);
    assert!(!refinement.matches(&other, None));
}

#[test]
fn a_family_without_refinements_ignores_the_scheduler() {
    let registry = PipelineFamilyRegistry::builtin();
    let scheduler = SchedulerFacts::new(
        Some("EulerAncestralDiscreteScheduler".to_owned()),
        Some("trailing".to_owned()),
    );
    // FLUX has no refinements; the scheduler/name are inert.
    let with = registry
        .profile("FluxPipeline", Some(&scheduler), Some("flux-turbo"))
        .expect("flux");
    let without = registry.profile("FluxPipeline", None, None).expect("flux");
    assert_eq!(with.params, without.params);
}
