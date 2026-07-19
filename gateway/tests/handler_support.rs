//! The `resolve_authorized` gate: it should resolve a ready, in-scope, admissible
//! model and reject on scope, admission, and not-found.

mod common;

use common::MockPort;
use gateway::admission::{GatewayAdmissionState, GatewayWorkKind};
use gateway::error::GatewayErrorKind;
use gateway::identity::GatewayIdentity;
use gateway::resolver::resolve_authorized;
use gateway::scopes::GatewayScopes;
use kernel::records::Capability;

fn identity(scopes: GatewayScopes) -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", scopes)
}

#[tokio::test]
async fn a_ready_in_scope_model_resolves() {
    let (port, id) = MockPort::with_ready_model("llama3");
    let record = resolve_authorized(
        &port,
        "llama3",
        Capability::chat(),
        GatewayWorkKind::Stream,
        &identity(GatewayScopes::all()),
    )
    .await
    .unwrap();
    assert_eq!(record.id, id);
}

#[tokio::test]
async fn an_out_of_scope_capability_is_forbidden() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let scopes = GatewayScopes {
        models: None,
        capabilities: Some(vec!["embed".to_owned()]),
    };
    let error = resolve_authorized(
        &port,
        "llama3",
        Capability::chat(),
        GatewayWorkKind::Stream,
        &identity(scopes),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, GatewayErrorKind::Forbidden);
}

#[tokio::test]
async fn a_saturated_machine_is_overloaded() {
    let (mut port, _id) = MockPort::with_ready_model("llama3");
    port.admission = GatewayAdmissionState::Saturated {
        retry_after_seconds: 1,
    };
    let error = resolve_authorized(
        &port,
        "llama3",
        Capability::chat(),
        GatewayWorkKind::Stream,
        &identity(GatewayScopes::all()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, GatewayErrorKind::Overloaded);
    assert_eq!(error.retry_after_seconds, Some(1));
}

#[tokio::test]
async fn an_unknown_model_is_not_found() {
    let (port, _id) = MockPort::with_ready_model("llama3");
    let error = resolve_authorized(
        &port,
        "mistral",
        Capability::chat(),
        GatewayWorkKind::Stream,
        &identity(GatewayScopes::all()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, GatewayErrorKind::NotFound);
}
