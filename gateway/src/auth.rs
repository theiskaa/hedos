//! Authenticating a request into a [`GatewayIdentity`]. The loopback default
//! treats every caller as an unrestricted local client; token-verified client
//! auth (backed by a client store) implements the same trait and slots in later.

use std::future::Future;
use std::pin::Pin;

use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::GatewayIdentity;
use crate::request::GatewayRequest;
use crate::scopes::GatewayScopes;

/// The future an [`Authenticating::authenticate`] returns.
pub type AuthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GatewayIdentity, GatewayError>> + Send + 'a>>;

/// Turns a request into the identity behind it, or rejects it.
pub trait Authenticating: Send + Sync {
    /// Authenticate `request`.
    fn authenticate<'a>(&'a self, request: &'a GatewayRequest) -> AuthFuture<'a>;
}

/// Loopback authentication: every caller is an unrestricted local client. Fits a
/// gateway bound to localhost for the machine's own tools; token-scoped auth
/// replaces it when a client store is configured.
pub struct OpenAuth;

impl Authenticating for OpenAuth {
    fn authenticate<'a>(&'a self, _request: &'a GatewayRequest) -> AuthFuture<'a> {
        Box::pin(async { Ok(GatewayIdentity::new("local", "local", GatewayScopes::all())) })
    }
}

/// Single-secret authentication: the request's bearer token (`Authorization:
/// Bearer …` or `x-api-key`) must match a pre-shared secret exactly, or the
/// request is rejected.
///
/// This is a prototype from plan 007 (see `plans/007-findings.md` for the
/// options considered and the recommendation). It is not wired into
/// `cli/src/support/serving.rs`; `OpenAuth` stays the default there.
pub struct TokenAuth {
    secret: String,
}

impl TokenAuth {
    /// A new `TokenAuth` requiring `secret` on every request.
    pub fn new(secret: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
        }
    }
}

impl Authenticating for TokenAuth {
    fn authenticate<'a>(&'a self, request: &'a GatewayRequest) -> AuthFuture<'a> {
        Box::pin(async move {
            // A request with no token at all is always rejected, even against
            // a degenerate empty-string secret: there is no bearer token to
            // authenticate, present or absent.
            let matches = request.bearer_token().is_some_and(|presented| {
                constant_time_eq(presented.as_bytes(), self.secret.as_bytes())
            });
            if matches {
                Ok(GatewayIdentity::new("local", "local", GatewayScopes::all()))
            } else {
                Err(GatewayError::new(
                    GatewayErrorKind::Unauthorized,
                    "missing or invalid API token",
                ))
            }
        })
    }
}

/// Compare two byte strings without branching on the first mismatching byte,
/// so a caller cannot learn how many leading bytes it guessed correctly from
/// timing. A length mismatch is still visible (there is no fixed-length secret
/// to pad against), but the loop below never returns early once it starts.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with(header: Option<(&str, &str)>) -> GatewayRequest {
        let headers = header
            .map(|(name, value)| vec![(name.to_owned(), value.to_owned())])
            .unwrap_or_default();
        GatewayRequest::new("GET", "/v1/models", headers, Vec::new())
    }

    #[tokio::test]
    async fn the_correct_bearer_token_is_accepted() {
        let auth = TokenAuth::new("s3cret");
        let request = request_with(Some(("Authorization", "Bearer s3cret")));
        let identity = auth.authenticate(&request).await.unwrap();
        assert_eq!(identity.client_id, "local");
        assert_eq!(identity.scopes, GatewayScopes::all());
    }

    #[tokio::test]
    async fn the_correct_api_key_header_is_accepted() {
        let auth = TokenAuth::new("s3cret");
        let request = request_with(Some(("x-api-key", "s3cret")));
        assert!(auth.authenticate(&request).await.is_ok());
    }

    #[tokio::test]
    async fn a_wrong_token_is_rejected() {
        let auth = TokenAuth::new("s3cret");
        let request = request_with(Some(("Authorization", "Bearer wrong")));
        let error = auth.authenticate(&request).await.unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn a_missing_token_is_rejected() {
        let auth = TokenAuth::new("s3cret");
        let request = request_with(None);
        let error = auth.authenticate(&request).await.unwrap_err();
        assert_eq!(error.kind, GatewayErrorKind::Unauthorized);
    }

    #[tokio::test]
    async fn an_empty_secret_does_not_match_an_absent_token() {
        // A degenerate config (empty secret) must not authenticate an
        // unauthenticated request just because both sides are empty strings.
        let auth = TokenAuth::new("");
        let request = request_with(None);
        assert!(auth.authenticate(&request).await.is_err());
    }

    #[test]
    fn constant_time_eq_matches_regular_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
