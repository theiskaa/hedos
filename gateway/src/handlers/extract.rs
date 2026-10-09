//! The extraction handler: `POST /v1/extract`, a hedos route neither OpenAI nor
//! Ollama has. The body is the request a contact extractor such as Tessera
//! reads (`operation`, `text`, and its options) with the `model` that should
//! answer it; the reply is the one JSON object the extractor wrote, returned as
//! it was written. A request the extractor refuses is a `400` with its reason.
//!
//! The model has to resolve, and to one that declares `extract`; anything else
//! is refused with the names of the models that would do.

use kernel::records::{Capability, JsonValue};

use super::named::{Wanted, named};
use super::{GatewayHandling, HandlerFuture, bad_request, collect_completion, server_error};
use crate::admission::GatewayWorkKind;
use crate::error::GatewayError;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::{GatewayPort, require_admission};
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;

/// The extraction handler.
pub struct ExtractHandler;

fn extractors() -> Wanted {
    Wanted {
        required: vec![Capability::extract()],
        plural: "extract contacts",
        singular: "extracts contacts",
    }
}

impl GatewayHandling for ExtractHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let (model, payload) = split_model(&request.body)?;
            let shelf = port.shelf().await;
            let record = named(&shelf, identity, &model, &extractors())?;
            identity.require(&record.id, &Capability::extract())?;
            require_admission(port, &record, GatewayWorkKind::Stream).await?;

            let mut stream = port
                .invoke(&record.id, Capability::extract(), payload)
                .await?;
            let (reply, _, _) = collect_completion(&mut stream).await?;
            if !serde_json::from_str::<serde_json::Value>(&reply)
                .is_ok_and(|reply| reply.is_object())
            {
                return Err(server_error(format!(
                    "{}'s reply was not one JSON object",
                    record.display_name()
                )));
            }
            responder.respond(200, "application/json", reply.into_bytes(), Vec::new());
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::extract()),
            ))
        })
    }
}

/// The `model` the body names, and the rest of the body as the extractor's
/// request.
fn split_model(body: &[u8]) -> Result<(String, JsonValue), GatewayError> {
    let mut object = match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(object)) => object,
        _ => return Err(bad_request("request body must be a JSON object")),
    };
    let model = match object.remove("model") {
        Some(serde_json::Value::String(model)) if !model.is_empty() => model,
        _ => return Err(bad_request("model is required")),
    };
    let payload = serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| bad_request(error.to_string()))?;
    Ok((model, payload))
}
