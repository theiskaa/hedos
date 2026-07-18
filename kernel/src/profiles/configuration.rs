//! Merging a model's saved configuration into a request: keeping only parameter
//! values the current schema still recognizes, and seeding the system prompt.

use std::collections::BTreeMap;

use crate::records::{Capability, JsonValue, ModelRecord};

/// The record's parameter values, each run through its spec: a value is kept
/// only when its key still has a spec in the current schema AND the value
/// coerces/clamps to that spec (see `ParamSpec::normalized`). Values for
/// vanished parameters and wrong-typed / unnormalizable values are dropped, so
/// nothing out-of-range or off-type can reach a runtime.
pub fn normalized_param_values(record: &ModelRecord) -> BTreeMap<String, JsonValue> {
    let mut kept = BTreeMap::new();
    for (key, value) in &record.param_values {
        if let Some(spec) = record.params.iter().find(|spec| &spec.key == key)
            && let Some(normalized) = spec.normalized(value)
        {
            kept.insert(key.clone(), normalized);
        }
    }
    kept
}

/// A copy of `record` with parameter values for vanished parameters removed.
pub fn dropping_vanished_param_values(record: &ModelRecord) -> ModelRecord {
    let mut record = record.clone();
    record.param_values = normalized_param_values(&record);
    record
}

/// Merge the record's configuration into a request `payload`.
///
/// For a chat capability, a system prompt (session override, else the record's
/// own, else `fallback_prompt`) is seeded into the `messages` array, and any
/// `appended_block` is folded in. Saved parameter values fill in keys the payload
/// does not already set. Non-object, non-null payloads pass through untouched.
pub fn merged(
    record: &ModelRecord,
    capability: &Capability,
    payload: JsonValue,
    fallback_prompt: Option<&str>,
    session_prompt: Option<&str>,
    appended_block: Option<&str>,
) -> JsonValue {
    let overrides = normalized_param_values(record);
    let (prompt, block) = if *capability == Capability::chat() {
        let prompt = match session_prompt {
            Some(session) => trimmed(Some(session)),
            None => trimmed(record.system_prompt.as_deref()).or_else(|| trimmed(fallback_prompt)),
        };
        (prompt, trimmed(appended_block))
    } else {
        (None, None)
    };

    if overrides.is_empty() && prompt.is_none() && block.is_none() {
        return payload;
    }

    let mut fields = match payload {
        JsonValue::Object(fields) => fields,
        JsonValue::Null => BTreeMap::new(),
        other => return other,
    };

    for (key, value) in overrides {
        fields.entry(key).or_insert(value);
    }

    if (prompt.is_some() || block.is_some())
        && let Some(messages) = fields.get_mut("messages")
        && let JsonValue::Array(turns) = messages
    {
        *messages = seeded(prompt.as_deref(), block.as_deref(), std::mem::take(turns));
    }

    JsonValue::Object(fields)
}

fn trimmed(prompt: Option<&str>) -> Option<String> {
    let cleaned = prompt?.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

fn seeded(prompt: Option<&str>, block: Option<&str>, mut turns: Vec<JsonValue>) -> JsonValue {
    let system_index = turns.iter().position(is_system_turn);

    if let Some(index) = system_index {
        let Some(block) = block else {
            return JsonValue::Array(turns);
        };
        if let JsonValue::Object(fields) = &mut turns[index] {
            let existing = match fields.get("content") {
                Some(JsonValue::String(content)) => Some(content.clone()),
                _ => None,
            };
            if let Some(existing) = existing {
                let joined = [existing.as_str(), block]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                fields.insert("content".to_owned(), JsonValue::String(joined));
            }
        }
        return JsonValue::Array(turns);
    }

    let content = [prompt, block]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    if content.is_empty() {
        return JsonValue::Array(turns);
    }
    let mut system = BTreeMap::new();
    system.insert("role".to_owned(), JsonValue::String("system".to_owned()));
    system.insert("content".to_owned(), JsonValue::String(content));
    let mut updated = Vec::with_capacity(turns.len() + 1);
    updated.push(JsonValue::Object(system));
    updated.extend(turns);
    JsonValue::Array(updated)
}

fn is_system_turn(turn: &JsonValue) -> bool {
    matches!(turn, JsonValue::Object(fields)
        if fields.get("role") == Some(&JsonValue::String("system".to_owned())))
}
