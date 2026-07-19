//! Authenticating a request into a [`GatewayIdentity`]. The loopback default
//! treats every caller as an unrestricted local client; token-verified client
//! auth (backed by a client store) implements the same trait and slots in later.

use std::future::Future;
use std::pin::Pin;

use crate::error::GatewayError;
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
