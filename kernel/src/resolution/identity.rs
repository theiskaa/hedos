//! The identity and bid foundation types: what `Identification::identify`
//! produces about a model ([`IdentifiedModel`]) and what a runtime adapter
//! offers to serve it ([`RuntimeBid`]). The `identify` orchestration and the bid
//! auction build on these.

use std::path::{Path, PathBuf};

use crate::discovery::gguf_models::is_mmproj_name;
use crate::discovery::modality_hints::{
    Hint, SentenceTransformersLayout, from_config_json, sentence_transformers_layout,
};
use crate::discovery::weights::{gguf_tree, primary_of};
use crate::records::{
    Capability, ExecutionMode, JsonValue, Modality, ModelRecord, ParamSpec, ParamType, RunTier,
    RuntimeId, SourceKind,
};
use crate::resolution::format::{
    GgufFacts, GgufPooling, ModelFormat, gguf_architecture_profile, ollama_profile,
};
use crate::resolution::gguf::{gguf_facts, has_ggml_magic, has_gguf_magic};
use crate::resolution::pipelines::{
    PipelineFamilyRegistry, SchedulerFacts, diffusers_pipeline_class,
};
use crate::resolution::safetensors::safetensors_format;

/// What identification determined about a model: its format and the modality,
/// capabilities, execution shape, parameter schema, and context/template facts
/// implied by it.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentifiedModel {
    /// The recognized format.
    pub format: ModelFormat,
    /// The modality, if determined.
    pub modality: Option<Modality>,
    /// The capabilities the model can serve.
    pub capabilities: Vec<Capability>,
    /// How the model executes.
    pub execution: ExecutionMode,
    /// The parameter schema for the model.
    pub params: Vec<ParamSpec>,
    /// The diffusers pipeline class, if any.
    pub pipeline_class: Option<String>,
    /// A context-window hint.
    pub context_length: Option<i64>,
    /// Whether the model ships a chat template.
    pub has_chat_template: Option<bool>,
    /// The quantization the weights carry, as their format names it.
    pub quantization: Option<String>,
}

impl IdentifiedModel {
    /// An identification with just the core fields; the rest default to empty.
    pub fn new(
        format: ModelFormat,
        modality: Option<Modality>,
        capabilities: Vec<Capability>,
        execution: ExecutionMode,
    ) -> Self {
        Self {
            format,
            modality,
            capabilities,
            execution,
            params: Vec::new(),
            pipeline_class: None,
            context_length: None,
            has_chat_template: None,
            quantization: None,
        }
    }
}

/// A runtime adapter's offer to serve a model: how well it runs (the tier), a
/// ranking preference (lower wins), and the other runtimes that could also serve
/// it (recorded as alternatives on the resolved record).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeBid {
    /// How the runtime would run the model.
    pub tier: RunTier,
    /// The ranking preference: a lower value wins. The `tier` is not part of the
    /// ordering (it is recorded on the winner, not compared).
    pub preference: i64,
    /// Other runtimes that could serve the model.
    pub alternatives: Vec<RuntimeId>,
}

impl RuntimeBid {
    /// A bid at `tier`/`preference` with no alternatives.
    pub fn new(tier: RunTier, preference: i64) -> Self {
        Self {
            tier,
            preference,
            alternatives: Vec::new(),
        }
    }

    /// A bid carrying the runtimes that could also serve the model.
    pub fn with_alternatives(tier: RunTier, preference: i64, alternatives: Vec<RuntimeId>) -> Self {
        Self {
            tier,
            preference,
            alternatives,
        }
    }
}

/// Identify what a `record` is from its source kind and on-disk files: a fixed
/// profile for builtin/endpoint/ollama models, else the GGUF/GGML header, a
/// diffusers `model_index.json`, a `config.json`+safetensors layout, or the
/// GGUF weights inside the container when the record names a directory, which
/// is the shape a Hugging Face cache repo has.
///
/// `record.source.path` is taken as-is (callers pass an absolute path — discovery
/// does); a leading `~` is not expanded.
pub fn identify(record: &ModelRecord) -> IdentifiedModel {
    let kind = &record.source.kind;
    if *kind == SourceKind::builtin() {
        return chat_model(ModelFormat::Builtin, builtin_params());
    }
    if *kind == SourceKind::endpoint() {
        return chat_model(ModelFormat::Endpoint, endpoint_params());
    }
    if *kind == SourceKind::ollama() {
        // The blob a manifest points at is a GGUF, so its header says what the
        // manifest does not: the architecture and the quantization.
        let facts = record
            .primary_weight_path
            .as_deref()
            .and_then(|path| gguf_facts(Path::new(path)));
        let profile = ollama_profile(
            manifest_has_projector_layer(&record.source.path),
            facts
                .as_ref()
                .and_then(|facts| facts.architecture.as_deref()),
        );
        let mut model = IdentifiedModel::new(
            ModelFormat::OllamaStore,
            Some(profile.modality),
            profile.capabilities,
            profile.execution,
        );
        model.quantization = facts.and_then(|facts| facts.quantization);
        return model;
    }

    let base = Path::new(&record.source.path);
    let container = container_url(base, record);
    let extension = base
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);

    if extension.as_deref() == Some("bin") && has_ggml_magic(base) {
        return IdentifiedModel::new(
            ModelFormat::GgmlBin,
            Some(Modality::audio()),
            vec![Capability::transcribe()],
            ExecutionMode::Stream,
        );
    }

    if is_gguf_file(base) {
        let has_projector = !projectors_of(base, mmproj_companions(base)).is_empty();
        return identify_gguf(base, has_projector);
    }

    let model_index = container.join("model_index.json");
    if model_index.exists() {
        return identify_diffusers(&model_index, &container, record);
    }

    let config = container.join("config.json");
    let hint = from_config_json(&config);
    if let Some(format) = safetensors_format(&container, &config) {
        return identify_safetensors(format, hint.as_ref(), &container);
    }
    // A repo whose weights are GGUF names a directory, so the file the header
    // is read from is inside it. Placed after the diffusers and safetensors
    // layouts so a directory that resolves as one keeps doing so, and before
    // the bare config hint below, which it deliberately outranks: a header
    // read from the weights says more than a sibling config file.
    if let Some((weights, has_projector)) =
        gguf_weights(&container, record.primary_weight_path.as_deref())
    {
        return identify_gguf(&weights, has_projector);
    }
    match hint {
        Some(hint) => {
            let mut model = IdentifiedModel::new(
                ModelFormat::Unknown,
                hint.modality,
                hint.capabilities,
                hint.execution,
            );
            model.context_length = hint.context_length;
            model
        }
        None => IdentifiedModel::new(ModelFormat::Unknown, None, Vec::new(), ExecutionMode::Sync),
    }
}

/// The GGUF file a model held in a directory is served from, when it is one,
/// and whether a projector sits with it. Which file that is comes from
/// [`primary_of`], the rule the store scanners pick a primary weight by, so the
/// header read here belongs to the file a server will load. Weights kept in a
/// subdirectory, as a repo with one folder per quantization keeps them, are
/// reached the same way, and so is a projector left at the root beside them.
///
/// The file is named through the container rather than through `primary`,
/// which a Hugging Face record resolves to a blob whose name carries nothing:
/// the projector checks read both the name and its neighbours.
///
/// `primary` refuses the whole container when it is not itself a GGUF, missing
/// included: of the formats this branch can answer, what the runtime would
/// load has the last word.
///
/// An architecture [`gguf_architecture_profile`] does not know reads as a text
/// chat model, which is the answer the same bytes get as a loose file.
fn gguf_weights(container: &Path, primary: Option<&str>) -> Option<(PathBuf, bool)> {
    if let Some(primary) = primary
        && !has_gguf_magic(Path::new(primary))
    {
        return None;
    }
    let tree = gguf_tree(container);
    let weights = primary_of(&tree.weights)?;
    let has_projector = !projectors_of(&weights, tree.projectors).is_empty();
    Some((weights, has_projector))
}

/// What a GGUF is, from its header. `has_projector` says whether a multimodal
/// projector accompanies it, which is what makes sight real: an architecture
/// that can see cannot without one.
fn identify_gguf(base: &Path, has_projector: bool) -> IdentifiedModel {
    let name = base
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if is_mmproj_name(name) {
        // A CLIP/mmproj projector: vision-only, no directly-served capability.
        return IdentifiedModel::new(
            ModelFormat::Gguf,
            Some(Modality::vision()),
            Vec::new(),
            ExecutionMode::Sync,
        );
    }
    let facts = gguf_facts(base);
    let decision = facts.as_ref().and_then(|facts| facts.decision.as_deref());
    let pooling = facts.as_ref().and_then(|facts| facts.pooling);
    let (modality, capabilities, execution) = match (decision, pooling) {
        // A decision model answers typed questions with probabilities and
        // nothing else, whatever its architecture, chat template or pooling
        // says: llama.cpp serves every decision type on `/v1/systemone` only.
        (Some(decision), _) => (
            Modality::text(),
            decision_capabilities(decision, has_projector),
            ExecutionMode::Stream,
        ),
        // A header that pools its states into one vector is an embedder,
        // whatever its architecture: llama.cpp writes the pooling of a decoder
        // converted to embed (Qwen3-Embedding) under the decoder's own name,
        // chat template and all.
        (None, Some(GgufPooling::Mean | GgufPooling::Cls | GgufPooling::Last)) => (
            Modality::embedding(),
            vec![Capability::embed()],
            ExecutionMode::Stream,
        ),
        // One relevance score per pair: a reranker, read the way a
        // cross-encoder in safetensors is.
        (None, Some(GgufPooling::Rank)) => (Modality::text(), Vec::new(), ExecutionMode::Stream),
        _ => profiled(facts.as_ref(), has_projector),
    };
    let mut model =
        IdentifiedModel::new(ModelFormat::Gguf, Some(modality), capabilities, execution);
    apply_facts(&mut model, facts.as_ref());
    model
}

/// The decision types whose prompt has a place for images, as llama.cpp's
/// `server_decision_context::can_use_images` names them. The others read text
/// alone, with or without a projector beside them.
const IMAGE_DECISION_TYPES: [&str; 2] = ["openjev", "clef"];

/// What a decision model of type `decision` does: it judges, and it sees when
/// its type reads images and a projector came with it.
fn decision_capabilities(decision: &str, has_projector: bool) -> Vec<Capability> {
    let mut capabilities = vec![Capability::judge()];
    if has_projector && IMAGE_DECISION_TYPES.contains(&decision) {
        capabilities.push(Capability::see());
    }
    capabilities
}

/// What a GGUF whose header pools nothing into one vector is: its
/// architecture's profile, or a text chat model when the table does not know
/// the architecture.
fn profiled(
    facts: Option<&GgufFacts>,
    has_projector: bool,
) -> (Modality, Vec<Capability>, ExecutionMode) {
    let Some(profile) = facts
        .and_then(|facts| facts.architecture.as_deref())
        .and_then(gguf_architecture_profile)
    else {
        let mut capabilities = vec![Capability::chat(), Capability::complete()];
        if has_projector {
            capabilities.push(Capability::see());
        }
        return (Modality::text(), capabilities, ExecutionMode::Stream);
    };
    let mut capabilities = profile.capabilities;
    let mut modality = profile.modality;
    if !has_projector {
        // The architecture can see, but no image encoder came with these
        // weights, so nothing here can read a picture. Sight is the pairing,
        // not the architecture.
        capabilities.retain(|capability| *capability != Capability::see());
    }
    if capabilities == [Capability::embed()] {
        // An encoder that pools nothing into one vector cannot embed through
        // llama.cpp, which pools nothing when the header names no pooling. One
        // carrying a classifier head is a reranker converted without its
        // pooling and claims nothing. What is left returns a state per token,
        // as the text encoder of a diffusion pipeline does: a component.
        capabilities.clear();
        if facts.is_some_and(|facts| facts.has_classifier_head) {
            modality = Modality::text();
        }
    }
    (modality, capabilities, profile.execution)
}

fn identify_safetensors(
    format: ModelFormat,
    hint: Option<&Hint>,
    container: &Path,
) -> IdentifiedModel {
    let hint_modality = hint.and_then(|hint| hint.modality.clone());
    let text = Some(Modality::text());
    if hint_modality.is_none() || hint_modality == text {
        let refined = match sentence_transformers_layout(container) {
            Some(SentenceTransformersLayout::Embedder) => {
                Some((Modality::embedding(), vec![Capability::embed()]))
            }
            // Its config names a causal LM, but what it reads out is one logit
            // per pair, never a reply, so the chat a config would imply is
            // withheld and a runtime that scores pairs has to claim it.
            Some(SentenceTransformersLayout::CrossEncoder) => Some((Modality::text(), Vec::new())),
            None => None,
        };
        if let Some((modality, capabilities)) = refined {
            let mut model =
                IdentifiedModel::new(format, Some(modality), capabilities, ExecutionMode::Stream);
            apply_hint(&mut model, hint);
            return model;
        }
    }
    let mut model = IdentifiedModel::new(
        format,
        hint.and_then(|hint| hint.modality.clone()),
        hint.map(|hint| hint.capabilities.clone())
            .unwrap_or_default(),
        hint.map_or(ExecutionMode::Sync, |hint| hint.execution),
    );
    apply_hint(&mut model, hint);
    model
}

/// Copy onto `model` what a GGUF header said about it. Every branch that reads
/// a header ends this way, so the set of facts a header carries is named once.
fn apply_facts(model: &mut IdentifiedModel, facts: Option<&GgufFacts>) {
    model.context_length = facts.and_then(|facts| facts.context_length);
    model.has_chat_template = facts.map(|facts| facts.has_chat_template);
    model.quantization = facts.and_then(|facts| facts.quantization.clone());
}

/// The same for what a `config.json` said, which is less: a config names no
/// chat template.
fn apply_hint(model: &mut IdentifiedModel, hint: Option<&Hint>) {
    model.context_length = hint.and_then(|hint| hint.context_length);
    model.quantization = hint.and_then(|hint| hint.quantization.clone());
}

fn chat_model(format: ModelFormat, params: Vec<ParamSpec>) -> IdentifiedModel {
    let mut model = IdentifiedModel::new(
        format,
        Some(Modality::text()),
        vec![Capability::chat(), Capability::complete()],
        ExecutionMode::Stream,
    );
    model.params = params;
    model
}

/// The snapshot directory for a Hugging Face cache record (its `ref` under
/// `snapshots/`, if present), else the base path itself.
fn container_url(base: &Path, record: &ModelRecord) -> PathBuf {
    if record.source.kind == SourceKind::huggingface_cache()
        && let Some(reference) = &record.source.reference
    {
        let snapshot = base.join("snapshots").join(reference);
        if snapshot.exists() {
            return snapshot;
        }
    }
    base.to_path_buf()
}

/// Whether an Ollama manifest at `path` declares a `.projector` (vision) layer.
fn manifest_has_projector_layer(path: &str) -> bool {
    let Ok(bytes) = crate::fs::read_regular(Path::new(path)) else {
        return false;
    };
    let Ok(JsonValue::Object(object)) = serde_json::from_slice::<JsonValue>(&bytes) else {
        return false;
    };
    let Some(JsonValue::Array(layers)) = object.get("layers") else {
        return false;
    };
    layers.iter().any(|layer| {
        layer
            .as_object()
            .and_then(|fields| fields.get("mediaType"))
            .and_then(JsonValue::as_str)
            .is_some_and(|media| media.ends_with(".projector"))
    })
}

/// Each sibling `mmproj` GGUF beside `base`, with its size in bytes.
fn mmproj_companions(base: &Path) -> Vec<(PathBuf, u64)> {
    let Some(directory) = base.parent() else {
        return Vec::new();
    };
    let base_name = base.file_name().and_then(|name| name.to_str());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str());
            name != base_name
                // Skip hidden files.
                && name.is_some_and(|name| !name.starts_with('.') && is_mmproj_name(name))
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        })
        .map(|path| {
            let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());
            (path, bytes)
        })
        .collect()
}

/// Whether `base` is a GGUF file, by its extension or its magic.
fn is_gguf_file(base: &Path) -> bool {
    base.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        || has_gguf_magic(base)
}

/// How a projector stands to the weights beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectorFit {
    /// It encodes images, one of its tensors has the weights' width, and the
    /// file holds its data: llama-server can load it with them.
    Fits,
    /// Its header or its tensors could not be read, or the file stops short
    /// of its data, as an unfinished download does.
    Unread,
    /// It encodes no images (an audio projector), or none of its tensors has
    /// the weights' width, so it belongs to another model.
    Misfit,
}

/// How `projector` stands to weights `width` wide. llama.cpp checks a
/// projector by the width of the tensors it outputs, not by any header key,
/// and an older converter wrote CLIP's own width (or 0) where a newer one
/// writes the text model's; so the width is looked for among the tensors.
fn projector_fit(width: Option<i64>, projector: &Path) -> ProjectorFit {
    let Some(facts) = gguf_facts(projector).filter(|facts| facts.architecture.is_some()) else {
        return ProjectorFit::Unread;
    };
    if !facts.has_vision_encoder {
        return ProjectorFit::Misfit;
    }
    let Some(dimensions) = facts.tensor_dimensions else {
        return ProjectorFit::Unread;
    };
    if let Some(width) = width.and_then(|width| u64::try_from(width).ok())
        && !dimensions.contains(&width)
    {
        return ProjectorFit::Misfit;
    }
    if facts.tensors_present {
        ProjectorFit::Fits
    } else {
        ProjectorFit::Unread
    }
}

/// The projectors among `candidates` that may be `weights`' sight, each with
/// how it fits them: every one but a misfit. One that could not be read is
/// kept, as its name is all identification has to go on, but never loaded.
fn projectors_of(
    weights: &Path,
    candidates: Vec<(PathBuf, u64)>,
) -> Vec<(PathBuf, u64, ProjectorFit)> {
    let width = gguf_facts(weights).and_then(|facts| facts.embedding_length);
    candidates
        .into_iter()
        .map(|(path, bytes)| {
            let fit = projector_fit(width, &path);
            (path, bytes, fit)
        })
        .filter(|(_, _, fit)| *fit != ProjectorFit::Misfit)
        .collect()
}

/// The multimodal projector a server loads with `record`'s GGUF weights, found
/// where identification looks for one, so the projector that made the record
/// see is the one it is served with: beside a loose file, or anywhere in a
/// snapshot's tree, and only one that fits the weights and holds its data. Among several (a repo
/// that ships the projector at more than one precision), the one named for the
/// weights' own quantization, else the smallest, since a projector's precision
/// costs memory and barely changes what it reads. `None` for a record that is
/// not a GGUF file or snapshot, or one with no projector.
pub fn projector_for(record: &ModelRecord) -> Option<PathBuf> {
    let kind = &record.source.kind;
    if *kind == SourceKind::ollama()
        || *kind == SourceKind::builtin()
        || *kind == SourceKind::endpoint()
    {
        return None;
    }
    let base = Path::new(&record.source.path);
    let (weights, candidates) = if is_gguf_file(base) {
        (base.to_path_buf(), mmproj_companions(base))
    } else {
        let tree = gguf_tree(&container_url(base, record));
        let weights = match &record.primary_weight_path {
            Some(path) => PathBuf::from(path),
            None => primary_of(&tree.weights)?,
        };
        (weights, tree.projectors)
    };
    let loadable: Vec<(PathBuf, u64)> = projectors_of(&weights, candidates)
        .into_iter()
        .filter(|(_, _, fit)| *fit == ProjectorFit::Fits)
        .map(|(path, bytes, _)| (path, bytes))
        .collect();
    choose_projector(&loadable, record.quantization.as_deref())
}

/// The projector [`projector_for`] takes out of `projectors` for weights of
/// `quantization`. The quantization has to be a whole part of the name between
/// dashes and dots, so `BF16` weights never take an `F16` projector.
fn choose_projector(projectors: &[(PathBuf, u64)], quantization: Option<&str>) -> Option<PathBuf> {
    let named_for_weights = |path: &Path| {
        let (Some(quantization), Some(name)) = (
            quantization,
            path.file_name().and_then(|name| name.to_str()),
        ) else {
            return false;
        };
        name.split(['-', '.'])
            .any(|part| part.eq_ignore_ascii_case(quantization))
    };
    let smallest = |candidates: &mut dyn Iterator<Item = &(PathBuf, u64)>| {
        candidates
            .min_by(|(left_path, left), (right_path, right)| {
                left.cmp(right).then_with(|| left_path.cmp(right_path))
            })
            .map(|(path, _)| path.clone())
    };
    smallest(
        &mut projectors
            .iter()
            .filter(|(path, _)| named_for_weights(path)),
    )
    .or_else(|| smallest(&mut projectors.iter()))
}

/// Identify a diffusers bundle from its `model_index.json` and the pipeline-family
/// registry: the `_class_name` selects a family whose modality/capabilities/params
/// (refined by the scheduler + repo name) become the identification. An unknown or
/// absent class falls back to a bare `Diffusers` job carrying just the class name.
fn identify_diffusers(
    model_index: &Path,
    container: &Path,
    record: &ModelRecord,
) -> IdentifiedModel {
    let pipeline_class = diffusers_pipeline_class(model_index);
    let scheduler = scheduler_facts(container);
    let repo_hint = record.source.repo.as_deref().unwrap_or(&record.name);
    let profile = pipeline_class.as_deref().and_then(|class| {
        PipelineFamilyRegistry::shared().profile(class, scheduler.as_ref(), Some(repo_hint))
    });
    let Some(profile) = profile else {
        let mut model =
            IdentifiedModel::new(ModelFormat::Diffusers, None, Vec::new(), ExecutionMode::Job);
        model.pipeline_class = pipeline_class;
        return model;
    };
    let mut params = profile.params;
    // FLUX schnell/dev differ: a distilled model that ignores guidance drops the
    // guidance parameter entirely rather than exposing a dead knob.
    if pipeline_class.as_deref() == Some("FluxPipeline") && !flux_uses_guidance(container) {
        params.retain(|spec| spec.key != "guidance");
    }
    let mut model = IdentifiedModel::new(
        ModelFormat::Diffusers,
        Some(profile.modality),
        profile.capabilities,
        ExecutionMode::Job,
    );
    model.params = params;
    model.pipeline_class = pipeline_class;
    model
}

/// The scheduler facts from `scheduler/scheduler_config.json`, if the file parses.
fn scheduler_facts(container: &Path) -> Option<SchedulerFacts> {
    let path = container.join("scheduler").join("scheduler_config.json");
    let bytes = crate::fs::read_regular(&path).ok()?;
    let JsonValue::Object(config) = serde_json::from_slice::<JsonValue>(&bytes).ok()? else {
        return None;
    };
    Some(SchedulerFacts::new(
        config
            .get("_class_name")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        config
            .get("timestep_spacing")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
    ))
}

/// Whether a FLUX pipeline's transformer declares `guidance_embeds` (a guidance-
/// distilled model), read from `transformer/config.json`.
fn flux_uses_guidance(container: &Path) -> bool {
    let path = container.join("transformer").join("config.json");
    let Ok(bytes) = crate::fs::read_regular(&path) else {
        return false;
    };
    let Ok(JsonValue::Object(config)) = serde_json::from_slice::<JsonValue>(&bytes) else {
        return false;
    };
    config
        .get("guidance_embeds")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn param(key: &str, param_type: ParamType, range: Option<Vec<JsonValue>>) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type,
        default_value: None,
        range,
        values: None,
    }
}

fn builtin_params() -> Vec<ParamSpec> {
    vec![
        param(
            "temperature",
            ParamType::Float,
            Some(vec![JsonValue::Double(0.0), JsonValue::Double(2.0)]),
        ),
        param(
            "top_p",
            ParamType::Float,
            Some(vec![JsonValue::Double(0.0), JsonValue::Double(1.0)]),
        ),
        param(
            "top_k",
            ParamType::Int,
            Some(vec![JsonValue::Int(0), JsonValue::Int(100)]),
        ),
        param(
            "max_tokens",
            ParamType::Int,
            Some(vec![JsonValue::Int(1), JsonValue::Int(4096)]),
        ),
        param("seed", ParamType::Int, None),
    ]
}

fn endpoint_params() -> Vec<ParamSpec> {
    vec![
        param(
            "temperature",
            ParamType::Float,
            Some(vec![JsonValue::Double(0.0), JsonValue::Double(2.0)]),
        ),
        param(
            "top_p",
            ParamType::Float,
            Some(vec![JsonValue::Double(0.0), JsonValue::Double(1.0)]),
        ),
        param(
            "max_tokens",
            ParamType::Int,
            Some(vec![JsonValue::Int(1), JsonValue::Int(32768)]),
        ),
        param("stop", ParamType::String, None),
        param("seed", ParamType::Int, None),
        param(
            "frequency_penalty",
            ParamType::Float,
            Some(vec![JsonValue::Double(-2.0), JsonValue::Double(2.0)]),
        ),
        param(
            "presence_penalty",
            ParamType::Float,
            Some(vec![JsonValue::Double(-2.0), JsonValue::Double(2.0)]),
        ),
    ]
}
