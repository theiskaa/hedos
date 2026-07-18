//! The parameter-schema system: which tunable parameters a model exposes,
//! assembled from capability- and runtime-specific profiles.

use crate::records::{Capability, JsonValue, ModelRecord, ParamSpec, ParamType, RuntimeId};

type Matcher = Box<dyn Fn(&ModelRecord) -> bool + Send + Sync>;

/// A set of parameter specs that applies to any model the `matches` predicate
/// accepts.
pub struct ModelProfile {
    /// A stable identifier for the profile.
    pub id: String,
    /// The parameter specs this profile contributes.
    pub schema: Vec<ParamSpec>,
    matcher: Matcher,
}

impl ModelProfile {
    /// Build a profile from an id, its schema, and a predicate.
    pub fn new(
        id: &str,
        schema: Vec<ParamSpec>,
        matcher: impl Fn(&ModelRecord) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.to_owned(),
            schema,
            matcher: Box::new(matcher),
        }
    }

    /// Whether this profile applies to `record`.
    pub fn matches(&self, record: &ModelRecord) -> bool {
        (self.matcher)(record)
    }
}

/// A collection of profiles that together determine a model's parameter schema.
pub struct ProfileRegistry {
    /// The profiles considered, in priority order.
    pub profiles: Vec<ModelProfile>,
}

impl ProfileRegistry {
    /// Build a registry from a list of profiles.
    pub fn new(profiles: Vec<ModelProfile>) -> Self {
        Self { profiles }
    }

    /// The parameter schema for `record`: every matching profile's specs (first
    /// occurrence of each key wins), plus a `context_length` spec when the model
    /// and runtime honor one.
    pub fn schema(&self, record: &ModelRecord) -> Vec<ParamSpec> {
        let mut specs: Vec<ParamSpec> = Vec::new();
        for profile in &self.profiles {
            if profile.matches(record) {
                for spec in &profile.schema {
                    if !specs.iter().any(|kept| kept.key == spec.key) {
                        specs.push(spec.clone());
                    }
                }
            }
        }
        if !specs.iter().any(|spec| spec.key == "context_length")
            && let Some(context) = context_length_spec(record)
        {
            specs.push(context);
        }
        specs
    }

    /// A copy of `record` with its parameter schema refreshed. An empty schema
    /// leaves the record unchanged.
    pub fn refreshed(&self, record: &ModelRecord) -> ModelRecord {
        let schema = self.schema(record);
        if schema.is_empty() {
            return record.clone();
        }
        let mut updated = record.clone();
        updated.params = schema;
        updated
    }

    /// The built-in profile set covering text generation, per-runtime sampling
    /// extras, speech, transcription, and togglable thinking.
    pub fn builtin() -> Self {
        Self::new(vec![
            ModelProfile::new(
                "text-generation",
                vec![temperature(), top_p(), max_tokens()],
                is_text,
            ),
            runtime_extras(
                "sampling-llama-cpp",
                RuntimeId::llama_cpp(),
                vec![
                    top_k(),
                    min_p(),
                    repeat_penalty(),
                    frequency_penalty(),
                    presence_penalty(),
                    seed(),
                    stop(),
                ],
            ),
            runtime_extras(
                "sampling-mlx-swift",
                RuntimeId::mlx_swift(),
                vec![repeat_penalty(), stop()],
            ),
            runtime_extras(
                "sampling-mlx-lm",
                RuntimeId::mlx_lm(),
                vec![top_k(), min_p(), repeat_penalty(), seed(), stop()],
            ),
            runtime_extras(
                "sampling-ollama",
                RuntimeId::ollama(),
                vec![
                    top_k(),
                    min_p(),
                    seed(),
                    repeat_penalty(),
                    frequency_penalty(),
                    presence_penalty(),
                    stop(),
                ],
            ),
            runtime_extras(
                "sampling-endpoint",
                RuntimeId::openai_endpoint(),
                vec![stop(), seed(), frequency_penalty(), presence_penalty()],
            ),
            runtime_extras(
                "sampling-apple",
                RuntimeId::apple_foundation(),
                vec![top_k(), seed()],
            ),
            ModelProfile::new(
                "speech-synthesis",
                vec![plain("voice", ParamType::String), speed()],
                |record| record.can(&Capability::speak()),
            ),
            ModelProfile::new(
                "transcription",
                vec![
                    plain("language", ParamType::String),
                    plain("translate", ParamType::Bool),
                ],
                |record| record.can(&Capability::transcribe()),
            ),
            ModelProfile::new(
                "togglable-thinking",
                vec![plain("thinking", ParamType::Bool)],
                |record| {
                    record.can(&Capability::chat())
                        && record.runtime.id.as_ref().is_some_and(is_thinking_runtime)
                },
            ),
        ])
    }
}

/// The `context_length` spec for a chat/completion model on a runtime that honors
/// it (llama.cpp or Ollama), sized to the model's declared window.
pub fn context_length_spec(record: &ModelRecord) -> Option<ParamSpec> {
    if !(record.can(&Capability::chat()) || record.can(&Capability::complete())) {
        return None;
    }
    let runtime = record.runtime.id.as_ref()?;
    if !is_context_honoring(runtime) {
        return None;
    }
    match record.context_length {
        Some(window) if window > 0 => Some(ParamSpec {
            key: "context_length".to_owned(),
            param_type: ParamType::Int,
            default_value: Some(JsonValue::Int(window.min(32768))),
            range: Some(vec![
                JsonValue::Int(512.min(window)),
                JsonValue::Int(window),
            ]),
            values: None,
        }),
        _ => Some(ParamSpec {
            key: "context_length".to_owned(),
            param_type: ParamType::Int,
            default_value: None,
            range: Some(vec![JsonValue::Int(512), JsonValue::Int(131072)]),
            values: None,
        }),
    }
}

fn is_text(record: &ModelRecord) -> bool {
    record.can(&Capability::chat()) || record.can(&Capability::complete())
}

fn is_context_honoring(runtime: &RuntimeId) -> bool {
    *runtime == RuntimeId::ollama() || *runtime == RuntimeId::llama_cpp()
}

fn is_thinking_runtime(runtime: &RuntimeId) -> bool {
    *runtime == RuntimeId::ollama() || *runtime == RuntimeId::mlx_lm()
}

fn runtime_extras(id: &str, runtime: RuntimeId, schema: Vec<ParamSpec>) -> ModelProfile {
    ModelProfile::new(id, schema, move |record| {
        is_text(record) && record.runtime.id.as_ref() == Some(&runtime)
    })
}

fn float(key: &str, low: f64, high: f64) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Float,
        default_value: None,
        range: Some(vec![JsonValue::Double(low), JsonValue::Double(high)]),
        values: None,
    }
}

fn int(key: &str, low: i64, high: i64) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Int,
        default_value: None,
        range: Some(vec![JsonValue::Int(low), JsonValue::Int(high)]),
        values: None,
    }
}

fn plain(key: &str, param_type: ParamType) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type,
        default_value: None,
        range: None,
        values: None,
    }
}

fn temperature() -> ParamSpec {
    float("temperature", 0.0, 2.0)
}
fn top_p() -> ParamSpec {
    float("top_p", 0.0, 1.0)
}
fn top_k() -> ParamSpec {
    int("top_k", 0, 100)
}
fn min_p() -> ParamSpec {
    float("min_p", 0.0, 1.0)
}
fn max_tokens() -> ParamSpec {
    int("max_tokens", 1, 32768)
}
fn repeat_penalty() -> ParamSpec {
    float("repeat_penalty", 0.5, 2.0)
}
fn frequency_penalty() -> ParamSpec {
    float("frequency_penalty", -2.0, 2.0)
}
fn presence_penalty() -> ParamSpec {
    float("presence_penalty", -2.0, 2.0)
}
fn seed() -> ParamSpec {
    plain("seed", ParamType::Int)
}
fn stop() -> ParamSpec {
    plain("stop", ParamType::String)
}
fn speed() -> ParamSpec {
    ParamSpec {
        key: "speed".to_owned(),
        param_type: ParamType::Float,
        default_value: Some(JsonValue::Double(1.0)),
        range: Some(vec![JsonValue::Double(0.5), JsonValue::Double(2.0)]),
        values: None,
    }
}
