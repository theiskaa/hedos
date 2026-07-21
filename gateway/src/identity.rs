//! The authenticated caller (`GatewayIdentity`) and the outcome a handler
//! reports for the audit log (`GatewayOutcome`).

use kernel::records::Capability;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::scopes::GatewayScopes;

/// An authenticated client: its stable id, display name, and access scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayIdentity {
    /// The client's stable id.
    pub client_id: String,
    /// The client's display name.
    pub name: String,
    /// What this client is allowed to reach.
    pub scopes: GatewayScopes,
}

impl GatewayIdentity {
    /// A new identity.
    pub fn new(
        client_id: impl Into<String>,
        name: impl Into<String>,
        scopes: GatewayScopes,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            name: name.into(),
            scopes,
        }
    }

    /// Ensure this identity is scoped for `capability` on `model_id`, or return a
    /// forbidden error.
    pub fn require(&self, model_id: &str, capability: &Capability) -> Result<(), GatewayError> {
        if self.scopes.permits(model_id, capability) {
            Ok(())
        } else {
            Err(GatewayError::new(
                GatewayErrorKind::Forbidden,
                format!(
                    "this token is not scoped for {} on that model",
                    capability.as_str()
                ),
            ))
        }
    }
}

/// The audit outcome label written for a successful request. Shared so the
/// producer here and consumers like [`crate::stats`] classify by the same string
/// and cannot silently drift apart.
pub(crate) const OK_OUTCOME: &str = "ok";

/// The result a handler reports for the audit log: the HTTP status, an outcome
/// label, and the model and capability that were served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayOutcome {
    /// The HTTP status returned.
    pub status: u16,
    /// The audit outcome label (e.g. `ok`, `not_found`).
    pub outcome: String,
    /// The model that served the request, if any.
    pub model: Option<String>,
    /// The capability that was exercised, if any.
    pub capability: Option<String>,
}

impl GatewayOutcome {
    /// The plain success outcome (200, `ok`) with no model or capability.
    pub fn ok() -> Self {
        Self {
            status: 200,
            outcome: OK_OUTCOME.to_owned(),
            model: None,
            capability: None,
        }
    }

    /// A success outcome naming the model and capability served.
    pub fn ok_for(model: Option<&str>, capability: Option<&Capability>) -> Self {
        Self {
            status: 200,
            outcome: OK_OUTCOME.to_owned(),
            model: model.map(str::to_owned),
            capability: capability.map(|capability| capability.as_str().to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(scopes: GatewayScopes) -> GatewayIdentity {
        GatewayIdentity::new("client-1", "Test Client", scopes)
    }

    #[test]
    fn an_unrestricted_identity_permits_any_capability() {
        let identity = identity(GatewayScopes::all());
        assert!(identity.require("model-1", &Capability::chat()).is_ok());
    }

    #[test]
    fn an_out_of_scope_capability_is_forbidden() {
        let scopes = GatewayScopes {
            models: None,
            capabilities: Some(vec!["chat".to_owned()]),
        };
        let error = identity(scopes)
            .require("model-1", &Capability::embed())
            .unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::Forbidden);
        assert!(error.message.contains("embed"));
    }

    #[test]
    fn an_out_of_scope_model_is_forbidden() {
        let scopes = GatewayScopes {
            models: Some(vec!["allowed".to_owned()]),
            capabilities: None,
        };
        assert!(
            identity(scopes)
                .require("denied", &Capability::chat())
                .is_err()
        );
    }

    #[test]
    fn the_ok_outcomes_carry_the_expected_fields() {
        let plain = GatewayOutcome::ok();
        assert_eq!(plain.status, 200);
        assert_eq!(plain.outcome, "ok");
        assert!(plain.model.is_none());

        let served = GatewayOutcome::ok_for(Some("llama3"), Some(&Capability::chat()));
        assert_eq!(served.model.as_deref(), Some("llama3"));
        assert_eq!(served.capability.as_deref(), Some("chat"));
    }
}
