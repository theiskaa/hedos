//! Shared decoding helpers for the OpenAI and Ollama request bodies: normalizing
//! the `stop` parameter and rejecting parameters the gateway doesn't honor.

use std::collections::BTreeMap;

use kernel::records::JsonValue;

use crate::error::{GatewayError, GatewayErrorKind};

/// Normalize a `stop` parameter into an array of strings.
///
/// A single string becomes a one-element array; an array must contain only
/// strings and, if `max_count` is set, no more than that many. Anything else is
/// a bad request. Absent (`None`) yields `None`.
pub fn stop(
    raw: Option<&JsonValue>,
    max_count: Option<usize>,
) -> Result<Option<JsonValue>, GatewayError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw {
        JsonValue::String(single) => Ok(Some(JsonValue::Array(vec![JsonValue::String(
            single.clone(),
        )]))),
        JsonValue::Array(list) => {
            if let Some(max) = max_count
                && list.len() > max
            {
                return Err(GatewayError::new(
                    GatewayErrorKind::BadRequest,
                    format!("stop accepts at most {max} sequences"),
                )
                .with_code("unsupported_parameter"));
            }
            let mut strings = Vec::with_capacity(list.len());
            for item in list {
                let JsonValue::String(text) = item else {
                    return Err(GatewayError::new(
                        GatewayErrorKind::BadRequest,
                        "stop must be a string or array of strings",
                    ));
                };
                strings.push(JsonValue::String(text.clone()));
            }
            Ok(Some(JsonValue::Array(strings)))
        }
        _ => Err(GatewayError::new(
            GatewayErrorKind::BadRequest,
            "stop must be a string or array of strings",
        )),
    }
}

/// Reject the first body key (in sorted order) that isn't in `honored`.
///
/// The `BTreeMap` iterates keys in sorted order, so the offending key reported is
/// deterministic and matches the Swift pass's `keys.sorted()`.
pub fn reject_unknown_keys(
    body: &BTreeMap<String, JsonValue>,
    honored: &[&str],
    label: &str,
) -> Result<(), GatewayError> {
    for key in body.keys() {
        if !honored.contains(&key.as_str()) {
            return Err(GatewayError::new(
                GatewayErrorKind::BadRequest,
                format!("the {label} '{key}' is not supported"),
            )
            .with_code("unsupported_parameter"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array(items: Vec<&str>) -> JsonValue {
        JsonValue::Array(
            items
                .into_iter()
                .map(|s| JsonValue::String(s.to_owned()))
                .collect(),
        )
    }

    #[test]
    fn a_missing_stop_is_none() {
        assert_eq!(stop(None, Some(4)).unwrap(), None);
    }

    #[test]
    fn a_single_string_becomes_a_one_element_array() {
        let value = JsonValue::String("END".to_owned());
        assert_eq!(
            stop(Some(&value), Some(4)).unwrap(),
            Some(array(vec!["END"]))
        );
    }

    #[test]
    fn an_array_of_strings_passes_through() {
        let value = array(vec!["a", "b"]);
        assert_eq!(
            stop(Some(&value), Some(4)).unwrap(),
            Some(array(vec!["a", "b"]))
        );
    }

    #[test]
    fn too_many_stop_sequences_is_rejected_with_a_code() {
        let value = array(vec!["a", "b", "c", "d", "e"]);
        let error = stop(Some(&value), Some(4)).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert_eq!(error.wire_code().as_deref(), Some("unsupported_parameter"));
        assert!(error.message.contains("at most 4"));
    }

    #[test]
    fn a_non_string_array_element_is_rejected() {
        let value = JsonValue::Array(vec![JsonValue::String("a".to_owned()), JsonValue::Int(2)]);
        let error = stop(Some(&value), None).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert!(error.message.contains("string or array of strings"));
    }

    #[test]
    fn a_non_string_non_array_stop_is_rejected() {
        let error = stop(Some(&JsonValue::Bool(true)), None).unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
    }

    #[test]
    fn no_max_count_permits_any_length() {
        let value = array(vec!["a", "b", "c", "d", "e", "f"]);
        assert!(stop(Some(&value), None).is_ok());
    }

    fn body(pairs: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn honored_keys_pass() {
        let honored: &[&str] = &["model", "messages"];
        let body = body(&[
            ("model", JsonValue::String("m".to_owned())),
            ("messages", JsonValue::Array(Vec::new())),
        ]);
        assert!(reject_unknown_keys(&body, honored, "parameter").is_ok());
    }

    #[test]
    fn the_first_unknown_key_in_sorted_order_is_rejected() {
        let honored: &[&str] = &["model"];
        let body = body(&[
            ("model", JsonValue::String("m".to_owned())),
            ("zeta", JsonValue::Bool(true)),
            ("alpha", JsonValue::Bool(true)),
        ]);
        let error = reject_unknown_keys(&body, honored, "parameter").unwrap_err();
        // 'alpha' sorts before 'zeta', so it's the one reported.
        assert!(error.message.contains("'alpha'"), "{}", error.message);
        assert_eq!(error.wire_code().as_deref(), Some("unsupported_parameter"));
    }
}
