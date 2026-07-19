//! Rejecting request parameters the model's runtime doesn't honor, so a client
//! isn't silently ignored. Structural keys the gateway adds itself are exempt.

use std::collections::{BTreeMap, HashSet};

use kernel::records::{JsonValue, RuntimeId};

use crate::error::{GatewayError, GatewayErrorKind};

/// Parameter keys the gateway injects itself, which are always allowed through.
const STRUCTURAL_KEYS: &[&str] = &["thinking"];

/// Reject the first parameter (in sorted order) the runtime doesn't honor.
///
/// Structural keys the gateway adds are skipped. The error names the runtime so
/// the client knows which backend rejected the parameter.
pub fn require(
    params: &BTreeMap<String, JsonValue>,
    honored: &HashSet<String>,
    runtime: Option<&RuntimeId>,
) -> Result<(), GatewayError> {
    for key in params.keys() {
        if STRUCTURAL_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !honored.contains(key) {
            let runtime_name = runtime.map(RuntimeId::as_str).unwrap_or("selected");
            return Err(GatewayError::new(
                GatewayErrorKind::BadRequest,
                format!(
                    "the parameter '{key}' is not supported by the {runtime_name} runtime serving this model"
                ),
            )
            .with_code("unsupported_parameter"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(keys: &[&str]) -> BTreeMap<String, JsonValue> {
        keys.iter()
            .map(|k| (k.to_string(), JsonValue::Null))
            .collect()
    }

    fn honored(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn all_honored_params_pass() {
        let result = require(
            &params(&["temperature", "top_p"]),
            &honored(&["temperature", "top_p"]),
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn a_structural_key_is_always_allowed() {
        // `thinking` is injected by the gateway, not the client, so it passes even
        // when the runtime doesn't list it.
        let result = require(&params(&["thinking"]), &honored(&[]), None);
        assert!(result.is_ok());
    }

    #[test]
    fn an_unhonored_param_is_rejected_and_names_the_runtime() {
        let error = require(
            &params(&["temperature"]),
            &honored(&[]),
            Some(&RuntimeId::llama_cpp()),
        )
        .unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert_eq!(error.wire_code().as_deref(), Some("unsupported_parameter"));
        assert!(error.message.contains(RuntimeId::llama_cpp().as_str()));
    }

    #[test]
    fn an_unknown_runtime_is_called_selected() {
        let error = require(&params(&["temperature"]), &honored(&[]), None).unwrap_err();
        assert!(error.message.contains("selected"));
    }

    #[test]
    fn the_first_unhonored_key_in_sorted_order_is_reported() {
        // `alpha` sorts before `zeta`; it's the one named.
        let error = require(&params(&["zeta", "alpha"]), &honored(&[]), None).unwrap_err();
        assert!(error.message.contains("'alpha'"), "{}", error.message);
    }
}
