//! Rendering an artifact's provenance into a one-line summary and a multi-line
//! detail form. Known schema params come first (in schema order), then every
//! remaining scalar param alphabetically; `prompt` is pulled out separately.

use crate::records::{JsonValue, ParamSpec};

use super::artifact::Artifact;

/// Provenance rendering helpers.
pub struct Provenance;

impl Provenance {
    /// A one-line summary: model, the scalar params, and the duration.
    pub fn line(artifact: &Artifact, schema: &[ParamSpec]) -> String {
        let mut parts = vec![artifact.model.clone()];
        for (key, value) in param_pairs(&artifact.params, schema) {
            parts.push(format!("{key} {value}"));
        }
        parts.push(Self::duration(artifact.duration_ms));
        parts.join(" · ")
    }

    /// A human duration: `"850 ms"`, `"1.2s"`, `"3m"`, or `"3m 5s"`.
    pub fn duration(ms: i64) -> String {
        if ms < 1000 {
            return format!("{ms} ms");
        }
        if ms < 60_000 {
            return format!("{:.1}s", ms as f64 / 1000.0);
        }
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1000;
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds}s")
        }
    }

    /// A multi-line detail form: model, runtime, capability, prompt, params, then
    /// duration and job.
    pub fn details(artifact: &Artifact, schema: &[ParamSpec]) -> String {
        let mut lines = vec![
            format!("model: {}", artifact.model),
            format!("runtime: {}", artifact.runtime),
            format!("capability: {}", artifact.capability.as_ref()),
        ];
        lines.extend(prompt_and_param_lines(&artifact.params, schema));
        lines.push(format!(
            "duration: {}",
            Self::duration(artifact.duration_ms)
        ));
        lines.push(format!("job: {}", artifact.job_id));
        lines.join("\n")
    }

    /// A detail form for a failed generation.
    pub fn failure_details(
        model: &str,
        error: &str,
        job_id: Option<&str>,
        params: &JsonValue,
        schema: &[ParamSpec],
    ) -> String {
        let mut lines = vec![format!("model: {model}"), format!("error: {error}")];
        if let Some(job_id) = job_id {
            lines.push(format!("job: {job_id}"));
        }
        lines.extend(prompt_and_param_lines(params, schema));
        lines.join("\n")
    }

    /// The `prompt` string of a params object, if present.
    pub fn prompt(params: &JsonValue) -> Option<String> {
        params
            .as_object()
            .and_then(|fields| fields.get("prompt"))
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    }
}

fn prompt_and_param_lines(params: &JsonValue, schema: &[ParamSpec]) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(prompt) = Provenance::prompt(params) {
        lines.push(format!("prompt: {prompt}"));
    }
    for (key, value) in param_pairs(params, schema) {
        lines.push(format!("{key}: {value}"));
    }
    lines
}

fn param_pairs(params: &JsonValue, schema: &[ParamSpec]) -> Vec<(String, String)> {
    let Some(fields) = params.as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<String> = schema
        .iter()
        .map(|spec| spec.key.clone())
        .filter(|key| fields.contains_key(key))
        .collect();
    let mut extras: Vec<String> = fields
        .keys()
        .filter(|key| *key != "prompt" && !keys.contains(key))
        .cloned()
        .collect();
    extras.sort();
    keys.append(&mut extras);
    keys.into_iter()
        // The schema may itself list "prompt"; it is rendered on its own line.
        .filter(|key| key != "prompt")
        .filter_map(|key| {
            let value = fields.get(&key)?;
            let rendered = scalar(value)?;
            Some((key, rendered))
        })
        .collect()
}

fn scalar(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::Int(raw) => Some(raw.to_string()),
        JsonValue::Double(raw) => Some(raw.to_string()),
        JsonValue::String(raw) => Some(raw.clone()),
        JsonValue::Bool(raw) => Some(raw.to_string()),
        _ => None,
    }
}
