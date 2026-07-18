//! Seed injection for job payloads. A job's seed is fixed at submit time so the
//! exact seed that executes is what lands in the artifact's provenance.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};

use crate::records::JsonValue;

/// Inject a random `seed` into `payload` when it is a JSON object (or null,
/// treated as empty) that carries no non-null `seed`. Anything else, and any
/// object that already pins a seed, is returned unchanged.
pub fn seeded(payload: &JsonValue) -> JsonValue {
    let Some(mut fields) = seedable_fields(payload) else {
        return payload.clone();
    };
    if let Some(seed) = fields.get("seed")
        && *seed != JsonValue::Null
    {
        return JsonValue::Object(fields);
    }
    fields.insert("seed".to_owned(), JsonValue::Int(random_seed()));
    JsonValue::Object(fields)
}

/// Re-seed `params` with a fresh seed guaranteed to differ from its current one
/// (used by "vary"). A non-object payload is returned unchanged.
pub fn reseeded(params: &JsonValue) -> JsonValue {
    let Some(mut fields) = seedable_fields(params) else {
        return params.clone();
    };
    let previous = fields.get("seed").cloned();
    let mut fresh = JsonValue::Int(random_seed());
    while Some(&fresh) == previous.as_ref() {
        fresh = JsonValue::Int(random_seed());
    }
    fields.insert("seed".to_owned(), fresh);
    JsonValue::Object(fields)
}

fn seedable_fields(payload: &JsonValue) -> Option<BTreeMap<String, JsonValue>> {
    match payload {
        JsonValue::Object(fields) => Some(fields.clone()),
        JsonValue::Null => Some(BTreeMap::new()),
        _ => None,
    }
}

/// A pseudo-random value in `0..u32::MAX`, matching the Swift `Int.random(in:
/// 0..<UInt32.max)` range. Drawn from `RandomState`'s OS-seeded hasher keys so
/// no `rand` crate is needed.
fn random_seed() -> i64 {
    let entropy = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (entropy % u32::MAX as u64) as i64
}
