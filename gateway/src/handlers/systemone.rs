//! The System One handler: TypeSafe's `POST /v1/systemone`, so a TypeSafe SDK
//! pointed at this gateway reaches a local model with no change on its side.
//!
//! The route is a translation at the edge of the chat path. A model that answers
//! typed questions here takes them as JSON in a user message and replies with
//! the System One envelope as text, so the handler wraps the request's question
//! in one message, runs it as chat, and returns the reply as the body. There is
//! no streamed form: System One is one request and one envelope.
//!
//! **Model routing.** The name in the body has to resolve, and to a model that
//! declares the `judge` capability. Anything else is refused with the names of
//! the models that would do, rather than answered by whatever else is on the
//! shelf: a chat model handed this question replies in prose, and a client that
//! asked for `jev-latest` and got a local model's opinion under that name would
//! have no way to tell. The SDK sends `jev-latest` unless told otherwise, so the
//! refusal is also what tells a caller to set `TYPESAFE_DEFAULT_MODEL`.
//!
//! **Errors** keep the OpenAI shape, `{"error": {"message": …}}`. The SDK reads
//! `error.message` out of a failed response, so it shows the text written here.

use kernel::records::{Capability, ModelRecord, ModelState};

use super::{GatewayHandling, HandlerFuture, bad_request, collect_completion, completion_id};
use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::{GatewayPort, require_admission};
use crate::request::GatewayRequest;
use crate::resolver::resolve;
use crate::responder::GatewayResponder;
use crate::wire::typesafe;

/// The response header the TypeSafe SDK reads a request id from, to quote in
/// its errors and logs.
const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";

/// The System One handler.
pub struct SystemOneHandler;

impl GatewayHandling for SystemOneHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let question = typesafe::decode_request(&request.body)?;
            let record = judge_named(port, identity, &question.model).await?;
            identity.require(&record.id, &Capability::judge())?;
            require_admission(port, &record, GatewayWorkKind::Stream).await?;

            // The question travels as chat because that is the wire the model's
            // runtime speaks; `judge` says the model can answer, not how.
            let mut stream = port
                .invoke(&record.id, Capability::chat(), question.chat_payload())
                .await?;
            let (reply, _, _) = collect_completion(&mut stream).await?;
            responder.respond(
                200,
                "application/json",
                typesafe::envelope(&reply)?,
                vec![(REQUEST_ID_HEADER.to_owned(), completion_id("req-"))],
            );
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::judge()),
            ))
        })
    }
}

/// The ready, in-scope model `requested` names, provided it answers typed
/// questions. A name that resolves to nothing is a `404` and one that resolves
/// to a model without `judge` is a `400`; both name the models that would do.
async fn judge_named(
    port: &dyn GatewayPort,
    identity: &GatewayIdentity,
    requested: &str,
) -> Result<ModelRecord, GatewayError> {
    let shelf = port.shelf().await;
    let judges = || {
        let mut names: Vec<&str> = shelf
            .iter()
            .filter(|record| record.state == ModelState::Ready)
            .filter(|record| identity.scopes.permits_model(&record.id))
            .filter(|record| record.capabilities.contains(&Capability::judge()))
            .map(ModelRecord::display_name)
            .collect();
        names.sort_unstable();
        if names.is_empty() {
            "no model on this machine answers typed questions".to_owned()
        } else {
            format!(
                "the models that answer typed questions here: {}",
                names.join(", ")
            )
        }
    };
    match resolve(requested, &shelf, &identity.scopes) {
        Ok(record) if record.capabilities.contains(&Capability::judge()) => Ok(record),
        Ok(record) => Err(bad_request(format!(
            "{} does not answer typed questions; {}",
            record.display_name(),
            judges()
        ))),
        Err(error) if error.kind == GatewayErrorKind::NotFound => Err(GatewayError::new(
            GatewayErrorKind::NotFound,
            format!("{}; {}", error.message, judges()),
        )),
        Err(error) => Err(error),
    }
}
