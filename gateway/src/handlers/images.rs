//! The OpenAI image-generation handler: `/v1/images/generations`. Unlike the
//! text endpoints this runs as a job — submit, drain the job's event stream to
//! its result artifacts, then return the single image inline as base64.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use kernel::jobs::JobEvent;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use super::{
    GatewayHandling, HandlerFuture, bad_request, required_model, respond_json, server_error,
};
use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;
use crate::wire::timestamp::now_unix_seconds;

/// Images never leave the machine, so a long-running local render is given a
/// generous ceiling before it is cancelled.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

/// `POST /v1/images/generations`.
///
/// A runaway render is bounded by [`RUN_TIMEOUT`], which cancels the job. A
/// client that disconnects mid-render is not yet propagated to a job cancel —
/// the borrowed port cannot be cancelled from a `Drop` (cancel is async); prompt
/// disconnect-cancel belongs in the server bridge that owns the port. Either way
/// the job still terminates on its own within the timeout.
pub struct OpenAIImagesHandler;

impl GatewayHandling for OpenAIImagesHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let body = request.decoded_json()?;
            let model = required_model(&body)?;
            let prompt = body
                .get("prompt")
                .and_then(JsonValue::as_str)
                .filter(|prompt| !prompt.is_empty())
                .ok_or_else(|| bad_request("prompt is required"))?;
            if matches!(body.get("n"), Some(JsonValue::Int(count)) if *count > 1) {
                return Err(bad_request("only one image per request is available"));
            }
            if matches!(body.get("response_format"), Some(JsonValue::String(format)) if format != "b64_json")
            {
                return Err(bad_request(
                    "only b64_json output is available — images never leave this machine",
                ));
            }

            let record = resolve_authorized(
                port,
                model,
                Capability::image(),
                GatewayWorkKind::Job,
                identity,
            )
            .await?;

            let mut payload = BTreeMap::new();
            payload.insert("prompt".to_owned(), JsonValue::String(prompt.to_owned()));
            if let Some(JsonValue::String(size)) = body.get("size") {
                payload.insert("size".to_owned(), JsonValue::String(size.clone()));
            }
            let job_id = port
                .submit(&record.id, Capability::image(), JsonValue::Object(payload))
                .await?;

            let mut events = port.job_events(&job_id).await;
            let drain = async {
                while let Some(event) = events.recv().await {
                    match event {
                        JobEvent::Done { result } => return Ok(result),
                        JobEvent::Failed { message } => return Err(server_error(message)),
                        JobEvent::Cancelled => {
                            return Err(server_error("generation was cancelled"));
                        }
                        _ => {}
                    }
                }
                // The stream closed with no terminal event; an empty result folds
                // into the "produced no image" error below.
                Ok(Vec::new())
            };
            let artifact_ids = match tokio::time::timeout(RUN_TIMEOUT, drain).await {
                Ok(result) => result?,
                Err(_) => {
                    port.cancel(&job_id).await;
                    return Err(GatewayError::new(
                        GatewayErrorKind::Timeout,
                        format!(
                            "image generation ran longer than {}s",
                            RUN_TIMEOUT.as_secs()
                        ),
                    ));
                }
            };

            let image = match artifact_ids.first() {
                Some(id) => port.artifact_data(id).await?,
                None => None,
            };
            let Some(bytes) = image else {
                return Err(server_error(format!("{} produced no image", record.name)));
            };
            respond_json(
                responder,
                &json!({
                    "created": now_unix_seconds(),
                    "data": [{ "b64_json": BASE64_STANDARD.encode(&bytes) }],
                }),
            );
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::image()),
            ))
        })
    }
}
