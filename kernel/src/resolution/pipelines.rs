//! The diffusers pipeline-family registry: maps a `model_index.json`
//! `_class_name` to a modality/capability/parameter profile, refined by the
//! model's scheduler and repo name. This is how a discovered diffusers bundle
//! gets its image/video/audio shape and its sampler parameter schema.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use crate::records::{Capability, JsonValue, Modality, ParamSpec, ParamType};

/// The `_class_name` declared in a diffusers `model_index.json` (or scheduler
/// config), if the file reads and parses as an object.
pub(crate) fn diffusers_pipeline_class(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let JsonValue::Object(index) = serde_json::from_slice::<JsonValue>(&bytes).ok()? else {
        return None;
    };
    index
        .get("_class_name")
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

/// The resolved shape of a diffusers pipeline: what it produces and the sampler
/// parameters it exposes.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffusersPipelineProfile {
    /// What the pipeline generates.
    pub modality: Modality,
    /// The capabilities it serves.
    pub capabilities: Vec<Capability>,
    /// Its tunable parameter schema.
    pub params: Vec<ParamSpec>,
}

/// The facts read from a pipeline's `scheduler/scheduler_config.json` that a
/// refinement matches against.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SchedulerFacts {
    /// The scheduler's `_class_name`.
    pub class_name: Option<String>,
    /// Its `timestep_spacing`.
    pub timestep_spacing: Option<String>,
}

impl SchedulerFacts {
    /// Scheduler facts from a class name and timestep spacing.
    pub fn new(class_name: Option<String>, timestep_spacing: Option<String>) -> Self {
        Self {
            class_name,
            timestep_spacing,
        }
    }
}

/// A conditional parameter override applied to a family when the model's
/// scheduler (and optionally its repo name) matches — e.g. the SDXL "turbo"
/// low-step preset.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineRefinement {
    /// Scheduler `_class_name`s this refinement applies to.
    pub scheduler_classes: HashSet<String>,
    /// A required `timestep_spacing`, if the refinement is spacing-specific.
    pub timestep_spacing: Option<String>,
    /// Repo-name tokens that must be present (any one), if name-gated.
    pub name_signals: HashSet<String>,
    /// The parameter specs to overlay (replacing by key, or appending).
    pub param_overrides: Vec<ParamSpec>,
}

impl PipelineRefinement {
    /// Whether this refinement applies given the scheduler `facts` and an optional
    /// `repo_hint`. The scheduler class must match; a set `timestep_spacing` must
    /// match; and if any `name_signals` are declared, a token of `repo_hint` must
    /// be among them.
    pub fn matches(&self, facts: &SchedulerFacts, repo_hint: Option<&str>) -> bool {
        let Some(class_name) = &facts.class_name else {
            return false;
        };
        if !self.scheduler_classes.contains(class_name) {
            return false;
        }
        if let Some(spacing) = &self.timestep_spacing
            && facts.timestep_spacing.as_deref() != Some(spacing.as_str())
        {
            return false;
        }
        if self.name_signals.is_empty() {
            return true;
        }
        let Some(repo_hint) = repo_hint else {
            return false;
        };
        let tokens = name_tokens(repo_hint);
        self.name_signals
            .iter()
            .any(|signal| tokens.contains(signal))
    }
}

/// The lowercase alphanumeric tokens of a repo name (`"SDXL-Turbo/v1"` →
/// `{"sdxl", "turbo", "v1"}`).
fn name_tokens(repo_hint: &str) -> HashSet<String> {
    repo_hint
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A family of diffusers pipeline classes that share a modality/capability/
/// parameter profile (e.g. all SDXL pipelines), plus any scheduler refinements.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineFamily {
    /// A stable family id (`"stable-diffusion-xl"`).
    pub id: String,
    /// The `_class_name`s that belong to this family.
    pub class_names: HashSet<String>,
    /// What the family generates.
    pub modality: Modality,
    /// The capabilities it serves (empty for edit/upscale/video/audio families).
    pub capabilities: Vec<Capability>,
    /// The base parameter schema.
    pub params: Vec<ParamSpec>,
    /// Scheduler-conditional parameter refinements.
    pub refinements: Vec<PipelineRefinement>,
}

impl PipelineFamily {
    fn new(
        id: &str,
        class_names: &[&str],
        modality: Modality,
        capabilities: Vec<Capability>,
        params: Vec<ParamSpec>,
        refinements: Vec<PipelineRefinement>,
    ) -> Self {
        Self {
            id: id.to_owned(),
            class_names: class_names.iter().map(|name| (*name).to_owned()).collect(),
            modality,
            capabilities,
            params,
            refinements,
        }
    }
}

/// The registry of diffusers pipeline families. Look up a `_class_name` to get
/// its [`DiffusersPipelineProfile`].
#[derive(Debug, Clone)]
pub struct PipelineFamilyRegistry {
    /// The known families.
    pub families: Vec<PipelineFamily>,
}

impl PipelineFamilyRegistry {
    /// A registry over `families`.
    pub fn new(families: Vec<PipelineFamily>) -> Self {
        Self { families }
    }

    /// The process-wide built-in registry, built once on first use. Prefer this
    /// over [`builtin`](Self::builtin) at call sites — identification runs it per
    /// model, and the table is immutable.
    pub fn shared() -> &'static Self {
        static BUILTIN: LazyLock<PipelineFamilyRegistry> =
            LazyLock::new(PipelineFamilyRegistry::builtin);
        &BUILTIN
    }

    /// The family owning `class_name`, if any.
    pub fn family(&self, class_name: &str) -> Option<&PipelineFamily> {
        self.families
            .iter()
            .find(|family| family.class_names.contains(class_name))
    }

    /// The resolved profile for `class_name`, applying the first scheduler
    /// refinement that matches `scheduler`/`repo_hint`. `None` if the class name
    /// is unknown.
    pub fn profile(
        &self,
        class_name: &str,
        scheduler: Option<&SchedulerFacts>,
        repo_hint: Option<&str>,
    ) -> Option<DiffusersPipelineProfile> {
        let family = self.family(class_name)?;
        let mut params = family.params.clone();
        if let Some(scheduler) = scheduler
            && let Some(refinement) = family
                .refinements
                .iter()
                .find(|refinement| refinement.matches(scheduler, repo_hint))
        {
            for overlay in &refinement.param_overrides {
                if let Some(index) = params.iter().position(|spec| spec.key == overlay.key) {
                    params[index] = overlay.clone();
                } else {
                    params.push(overlay.clone());
                }
            }
        }
        Some(DiffusersPipelineProfile {
            modality: family.modality.clone(),
            capabilities: family.capabilities.clone(),
            params,
        })
    }

    /// The built-in family table covering the common diffusers pipelines.
    pub fn builtin() -> Self {
        Self::new(vec![
            PipelineFamily::new(
                "flux",
                &["FluxPipeline"],
                Modality::image(),
                vec![Capability::image()],
                flux_params(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "stable-diffusion",
                &["StableDiffusionPipeline"],
                Modality::image(),
                vec![Capability::image()],
                sd1_params(),
                vec![turbo_refinement()],
            ),
            PipelineFamily::new(
                "stable-diffusion-xl",
                &["StableDiffusionXLPipeline"],
                Modality::image(),
                vec![Capability::image()],
                sdxl_params(),
                vec![turbo_refinement()],
            ),
            PipelineFamily::new(
                "stable-diffusion-3",
                &["StableDiffusion3Pipeline"],
                Modality::image(),
                vec![Capability::image()],
                sd3_params(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "pixart",
                &["PixArtAlphaPipeline", "PixArtSigmaPipeline"],
                Modality::image(),
                vec![Capability::image()],
                pixart_params(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "kandinsky",
                &["KandinskyV22Pipeline", "KandinskyV22CombinedPipeline"],
                Modality::image(),
                vec![Capability::image()],
                kandinsky_params(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "latent-consistency",
                &["LatentConsistencyModelPipeline"],
                Modality::image(),
                vec![Capability::image()],
                lcm_params(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "image-edit",
                &[
                    "StableDiffusionImg2ImgPipeline",
                    "StableDiffusionInpaintPipeline",
                    "StableDiffusionXLImg2ImgPipeline",
                    "StableDiffusionXLInpaintPipeline",
                    "StableDiffusion3Img2ImgPipeline",
                    "StableDiffusion3InpaintPipeline",
                    "FluxImg2ImgPipeline",
                    "FluxInpaintPipeline",
                    "KandinskyV22Img2ImgPipeline",
                    "LatentConsistencyModelImg2ImgPipeline",
                ],
                Modality::image(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "image-upscale",
                &[
                    "StableDiffusionUpscalePipeline",
                    "StableDiffusionLatentUpscalePipeline",
                ],
                Modality::image(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "video",
                &[
                    "TextToVideoSDPipeline",
                    "AnimateDiffPipeline",
                    "CogVideoXPipeline",
                    "StableVideoDiffusionPipeline",
                    "HunyuanVideoPipeline",
                    "LTXPipeline",
                    "WanPipeline",
                ],
                Modality::video(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            PipelineFamily::new(
                "audio",
                &[
                    "AudioLDMPipeline",
                    "AudioLDM2Pipeline",
                    "MusicLDMPipeline",
                    "StableAudioPipeline",
                ],
                Modality::audio(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        ])
    }
}

fn int_param(key: &str, default: i64, min: i64, max: i64) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Int,
        default_value: Some(JsonValue::Int(default)),
        range: Some(vec![JsonValue::Int(min), JsonValue::Int(max)]),
        values: None,
    }
}

fn float_param(key: &str, default: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Float,
        default_value: Some(JsonValue::Double(default)),
        range: Some(vec![JsonValue::Double(min), JsonValue::Double(max)]),
        values: None,
    }
}

fn size_param(default: &str, values: &[&str]) -> ParamSpec {
    ParamSpec {
        key: "size".to_owned(),
        param_type: ParamType::Enum,
        default_value: Some(JsonValue::String(default.to_owned())),
        range: None,
        values: Some(values.iter().map(|value| (*value).to_owned()).collect()),
    }
}

/// A bare parameter with only a key and type (no default/range/values) — `seed`
/// and `negative_prompt`.
fn bare_param(key: &str, param_type: ParamType) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type,
        default_value: None,
        range: None,
        values: None,
    }
}

fn seed() -> ParamSpec {
    bare_param("seed", ParamType::Int)
}

fn negative_prompt() -> ParamSpec {
    bare_param("negative_prompt", ParamType::String)
}

fn flux_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 4, 1, 50),
        float_param("guidance", 4.0, 0.0, 10.0),
        size_param("1024x1024", &["512x512", "768x768", "1024x1024"]),
        seed(),
    ]
}

fn sd1_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 30, 1, 75),
        float_param("guidance", 7.5, 0.0, 15.0),
        size_param("512x512", &["512x512", "576x576", "640x640", "768x768"]),
        seed(),
        negative_prompt(),
    ]
}

fn sdxl_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 30, 1, 75),
        float_param("guidance", 7.0, 0.0, 15.0),
        size_param(
            "1024x1024",
            &["768x768", "1024x1024", "1152x896", "896x1152"],
        ),
        seed(),
        negative_prompt(),
    ]
}

fn sd3_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 28, 1, 75),
        float_param("guidance", 7.0, 0.0, 15.0),
        size_param(
            "1024x1024",
            &["768x768", "1024x1024", "1152x896", "896x1152"],
        ),
        seed(),
        negative_prompt(),
    ]
}

fn pixart_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 20, 1, 75),
        float_param("guidance", 4.5, 0.0, 15.0),
        size_param("1024x1024", &["512x512", "768x768", "1024x1024"]),
        seed(),
        negative_prompt(),
    ]
}

fn kandinsky_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 30, 1, 75),
        float_param("guidance", 4.0, 0.0, 15.0),
        size_param("768x768", &["512x512", "768x768", "1024x1024"]),
        seed(),
        negative_prompt(),
    ]
}

fn lcm_params() -> Vec<ParamSpec> {
    vec![
        int_param("steps", 4, 1, 8),
        float_param("guidance", 1.5, 0.0, 2.0),
        size_param("512x512", &["512x512", "768x768"]),
        seed(),
    ]
}

/// The "turbo/lightning/lcm" low-step refinement shared by the SD and SDXL
/// families: fewer steps and (near-)zero guidance under the trailing-spacing
/// ancestral scheduler.
fn turbo_refinement() -> PipelineRefinement {
    PipelineRefinement {
        scheduler_classes: ["EulerAncestralDiscreteScheduler"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        timestep_spacing: Some("trailing".to_owned()),
        name_signals: ["turbo", "lightning", "lcm"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        param_overrides: vec![
            int_param("steps", 2, 1, 8),
            float_param("guidance", 0.0, 0.0, 2.0),
        ],
    }
}
