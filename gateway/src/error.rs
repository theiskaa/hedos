//! The gateway's error type and how it renders on each wire surface.
//!
//! A [`GatewayError`] carries an HTTP status, an audit label, and the wire
//! `type`/`code` strings OpenAI clients expect, and serializes to the right
//! shape for whichever [`GatewaySurface`] served the request.

use runtime::facade::KernelError;
use serde_json::{Value, json};

use crate::surface::GatewaySurface;

/// The class of failure, which fixes the HTTP status and the wire vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewayErrorKind {
    /// The request was malformed or invalid (400).
    BadRequest,
    /// Authentication is missing or wrong (401).
    Unauthorized,
    /// The caller is authenticated but not permitted (403).
    Forbidden,
    /// No such model, artifact, or route (404).
    NotFound,
    /// The route exists but not for this method (405).
    MethodNotAllowed,
    /// The capability or feature isn't supported (501).
    NotSupported,
    /// The runtime is saturated; retry later (503).
    Overloaded,
    /// The request outran its deadline (504).
    Timeout,
    /// An unexpected internal failure (500).
    ServerError,
}

impl GatewayErrorKind {
    /// The stable snake-case label for this kind.
    pub fn label(self) -> &'static str {
        match self {
            Self::BadRequest => "invalid_request_error",
            Self::Unauthorized => "authentication_error",
            Self::Forbidden => "permission_error",
            Self::NotFound => "not_found_error",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::NotSupported => "not_supported",
            Self::Overloaded => "overloaded",
            Self::Timeout => "timeout_error",
            Self::ServerError => "api_error",
        }
    }

    /// The Anthropic `error.type` for this kind. Anthropic names a smaller set
    /// of types than the gateway distinguishes, so several kinds share one.
    pub fn anthropic_type(self) -> &'static str {
        match self {
            Self::BadRequest | Self::MethodNotAllowed | Self::NotSupported => {
                "invalid_request_error"
            }
            Self::Unauthorized => "authentication_error",
            Self::Forbidden => "permission_error",
            Self::NotFound => "not_found_error",
            Self::Overloaded => "overloaded_error",
            Self::Timeout | Self::ServerError => "api_error",
        }
    }

    /// The HTTP status code to return.
    pub fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::NotSupported => 501,
            Self::Overloaded => 503,
            Self::Timeout => 504,
            Self::ServerError => 500,
        }
    }

    /// The label recorded in the audit log for this outcome.
    pub fn audit_outcome(self) -> &'static str {
        match self {
            Self::BadRequest | Self::MethodNotAllowed => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::NotSupported => "not_supported",
            Self::Overloaded => "saturated",
            Self::Timeout => "timeout",
            Self::ServerError => "error",
        }
    }

    /// The OpenAI `error.type` string. Several kinds collapse onto `api_error`.
    fn wire_type(self) -> &'static str {
        match self {
            Self::BadRequest | Self::MethodNotAllowed => "invalid_request_error",
            Self::Unauthorized => "authentication_error",
            Self::Forbidden => "permission_error",
            Self::NotFound => "not_found_error",
            Self::NotSupported | Self::Overloaded | Self::Timeout | Self::ServerError => {
                "api_error"
            }
        }
    }

    /// The default OpenAI `error.code` when the error carries none of its own.
    fn default_wire_code(self) -> Option<&'static str> {
        match self {
            Self::MethodNotAllowed => Some("method_not_allowed"),
            Self::NotSupported => Some("capability_unsupported"),
            Self::Timeout => Some("timeout"),
            Self::Overloaded => Some("overloaded"),
            Self::BadRequest
            | Self::Unauthorized
            | Self::Forbidden
            | Self::NotFound
            | Self::ServerError => None,
        }
    }
}

/// A gateway failure carrying everything needed to render a wire response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GatewayError {
    /// The class of failure.
    pub kind: GatewayErrorKind,
    /// The human-readable message shown to the client.
    pub message: String,
    /// An explicit OpenAI `error.code`, overriding the kind's default.
    pub code: Option<String>,
    /// A `Retry-After` hint, in seconds, for saturation and timeouts.
    pub retry_after_seconds: Option<u32>,
}

impl GatewayError {
    /// A new error of `kind` with `message` and no code or retry hint.
    pub fn new(kind: GatewayErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            retry_after_seconds: None,
        }
    }

    /// Set an explicit OpenAI `error.code`.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Set a `Retry-After` hint in seconds.
    pub fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
    }

    /// The HTTP status code for this error.
    pub fn status(&self) -> u16 {
        self.kind.status()
    }

    /// The audit-log outcome label for this error.
    pub fn audit_outcome(&self) -> &'static str {
        self.kind.audit_outcome()
    }

    /// The OpenAI `error.code`: an explicit `code`, else the kind's default.
    pub fn wire_code(&self) -> Option<String> {
        self.code
            .clone()
            .or_else(|| self.kind.default_wire_code().map(str::to_owned))
    }

    /// The JSON body for this error on `surface`.
    pub fn body(&self, surface: GatewaySurface) -> Value {
        match surface {
            GatewaySurface::Ollama => json!({ "error": self.message }),
            // Claude Code string-matches on the upstream's error wording to
            // decide whether to retry and which capability to drop, so this
            // shape and the message must reach it unwrapped.
            GatewaySurface::Anthropic => json!({
                "type": "error",
                "error": {
                    "type": self.kind.anthropic_type(),
                    "message": self.message,
                },
            }),
            GatewaySurface::OpenAI => {
                let mut error = json!({
                    "message": self.message,
                    "type": self.kind.wire_type(),
                });
                if let Some(code) = self.wire_code() {
                    error["code"] = Value::String(code);
                }
                json!({ "error": error })
            }
        }
    }

    /// The serialized JSON body bytes for this error on `surface`.
    pub fn body_bytes(&self, surface: GatewaySurface) -> Vec<u8> {
        serde_json::to_vec(&self.body(surface)).unwrap_or_default()
    }
}

/// Map a kernel failure onto the wire, choosing a status and, for internal
/// failures, a generic message that doesn't leak runtime internals.
impl From<KernelError> for GatewayError {
    fn from(error: KernelError) -> Self {
        match &error {
            KernelError::ModelNotFound(_) | KernelError::ArtifactNotFound(_) => {
                GatewayError::new(GatewayErrorKind::NotFound, error.to_string())
            }
            KernelError::CapabilityUnsupported { .. } | KernelError::PayloadInvalid(_) => {
                GatewayError::new(GatewayErrorKind::BadRequest, error.to_string())
            }
            KernelError::ContextExceeded { .. } => {
                GatewayError::new(GatewayErrorKind::BadRequest, error.to_string())
                    .with_code("context_length_exceeded")
            }
            // Surfaced rather than hidden: on a loopback gateway that already
            // grants every local caller the whole shelf, the text of a runtime
            // failure is not what needs protecting, and it is the only thing
            // that explains a stopped daemon or an out-of-memory GPU.
            KernelError::RuntimeFailed(_) => {
                GatewayError::new(GatewayErrorKind::ServerError, error.to_string())
            }
            KernelError::Storage(_) => {
                GatewayError::new(GatewayErrorKind::ServerError, "internal error")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::Capability;

    #[test]
    fn status_and_labels_track_the_kind() {
        assert_eq!(GatewayErrorKind::NotFound.status(), 404);
        assert_eq!(GatewayErrorKind::Overloaded.status(), 503);
        assert_eq!(GatewayErrorKind::NotSupported.status(), 501);
        assert_eq!(
            GatewayErrorKind::BadRequest.label(),
            "invalid_request_error"
        );
        assert_eq!(
            GatewayErrorKind::MethodNotAllowed.audit_outcome(),
            "bad_request"
        );
        assert_eq!(GatewayErrorKind::Overloaded.audit_outcome(), "saturated");
    }

    #[test]
    fn method_not_allowed_collapses_its_wire_type_but_keeps_a_code() {
        let error = GatewayError::new(GatewayErrorKind::MethodNotAllowed, "nope");
        // The wire type collapses onto invalid_request_error...
        assert_eq!(error.kind.wire_type(), "invalid_request_error");
        // ...while the code preserves the distinction.
        assert_eq!(error.wire_code().as_deref(), Some("method_not_allowed"));
    }

    #[test]
    fn an_explicit_code_overrides_the_kind_default() {
        let error = GatewayError::new(GatewayErrorKind::BadRequest, "too long")
            .with_code("context_length_exceeded");
        assert_eq!(
            error.wire_code().as_deref(),
            Some("context_length_exceeded")
        );
        // A kind with no default code and no explicit code has none.
        let plain = GatewayError::new(GatewayErrorKind::BadRequest, "bad");
        assert_eq!(plain.wire_code(), None);
    }

    #[test]
    fn the_openai_body_nests_under_an_error_object() {
        let error = GatewayError::new(GatewayErrorKind::NotFound, "no model");
        let body = error.body(GatewaySurface::OpenAI);
        assert_eq!(body["error"]["message"], "no model");
        assert_eq!(body["error"]["type"], "not_found_error");
        // NotFound has no default code, so the field is absent.
        assert!(body["error"].get("code").is_none());
    }

    #[test]
    fn the_openai_body_includes_a_code_when_present() {
        let error = GatewayError::new(GatewayErrorKind::NotSupported, "no");
        let body = error.body(GatewaySurface::OpenAI);
        assert_eq!(body["error"]["code"], "capability_unsupported");
    }

    #[test]
    fn the_ollama_body_is_a_flat_error_string() {
        let error = GatewayError::new(GatewayErrorKind::ServerError, "boom");
        let body = error.body(GatewaySurface::Ollama);
        assert_eq!(body, json!({ "error": "boom" }));
    }

    #[test]
    fn not_found_kernel_errors_map_to_404() {
        let error: GatewayError = KernelError::ModelNotFound("m".to_owned()).into();
        assert_eq!(error.kind, GatewayErrorKind::NotFound);
        let error: GatewayError = KernelError::ArtifactNotFound("a".to_owned()).into();
        assert_eq!(error.kind, GatewayErrorKind::NotFound);
    }

    #[test]
    fn client_kernel_errors_map_to_400() {
        let error: GatewayError = KernelError::PayloadInvalid("bad".to_owned()).into();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert_eq!(error.message, "bad");
        let error: GatewayError = KernelError::CapabilityUnsupported {
            model: "m".to_owned(),
            capability: Capability::embed(),
        }
        .into();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
    }

    #[test]
    fn a_context_exceeded_error_carries_the_openai_code() {
        let error: GatewayError = KernelError::ContextExceeded {
            model: "m".to_owned(),
        }
        .into();
        assert_eq!(error.kind, GatewayErrorKind::BadRequest);
        assert_eq!(
            error.wire_code().as_deref(),
            Some("context_length_exceeded")
        );
    }

    #[test]
    fn a_runtime_failure_explains_itself_but_a_storage_failure_stays_generic() {
        // A runtime failure is the caller's to act on — a stopped daemon, a
        // model that cannot do tool calling, an out-of-memory GPU — and none of
        // it is secret from a local caller who can already reach every model on
        // the shelf. Hiding it leaves "something failed" as the only diagnosis.
        let error: GatewayError =
            KernelError::RuntimeFailed("ollama: model does not support tools".to_owned()).into();
        assert_eq!(error.kind, GatewayErrorKind::ServerError);
        assert!(error.message.contains("does not support tools"));

        // Storage failures stay generic: they describe this machine's disk
        // state, which the caller cannot act on and did not ask about.
        let error: GatewayError = KernelError::Storage("disk /y full".to_owned()).into();
        assert_eq!(error.kind, GatewayErrorKind::ServerError);
        assert!(!error.message.contains("disk"));
    }
}
