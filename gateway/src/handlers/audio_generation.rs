//! Audio generation: `/v1/audio/generations`, with `/v1/audio/music-generations`
//! as an alias. Neither OpenAI nor Ollama defines a route for music, so this
//! takes the request field names mlx-serve's music API uses (`prompt`,
//! `lyrics`, `instrumental`, `duration_seconds`, `seed`, ...); its streamed
//! events are hedos's own.
//!
//! The request is checked against the parameters the model takes
//! ([`audio_payload`]), so a key it has no use for is a `400` rather than
//! dropped, then run as a job. The answer is the finished file, or with
//! `"stream": true` server-sent `progress` events and a final `complete` (or
//! `error`) event carrying the file as base64. A song can take many minutes on
//! a laptop, so the ceiling is an hour, and a client that goes away cancels the
//! job: nobody would hear the result.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use kernel::capabilities::audio_payload;
use kernel::jobs::JobEvent;
use kernel::records::{Capability, JsonValue, ModelRecord};
use serde_json::json;

use super::{GatewayHandling, HandlerFuture, bad_request, required_model, server_error};
use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::{GatewayResponder, GatewayStreamBody};
use crate::wire::timestamp::now_unix_seconds;

/// How long one piece may take before its job is cancelled.
const RUN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// `POST /v1/audio/generations` and `POST /v1/audio/music-generations`.
pub struct AudioGenerationHandler;

impl GatewayHandling for AudioGenerationHandler {
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
            let stream = match body.get("stream") {
                None | Some(JsonValue::Null) => false,
                Some(JsonValue::Bool(stream)) => *stream,
                Some(_) => return Err(bad_request("stream should be true or false")),
            };
            let fields = request_fields(&body)?;

            let record = resolve_authorized(
                port,
                model,
                Capability::audio(),
                GatewayWorkKind::Job,
                identity,
            )
            .await?;
            refuse_unless_generator(&record)?;
            let payload = audio_payload(record.display_name(), &fields, &record.params)
                .map_err(|error| bad_request(error.to_string()))?;
            let format = payload
                .as_object()
                .and_then(|payload| payload.get("format"))
                .and_then(JsonValue::as_str)
                .unwrap_or("flac")
                .to_owned();

            let job_id = port
                .submit(&record.id, Capability::audio(), payload)
                .await?;
            // Until the job ends on its own, whatever ends this request (the
            // client leaving, the timeout, the server cutting it) cancels it.
            let mut guard = JobGuard {
                port,
                job_id: &job_id,
                armed: true,
            };
            let mut events = port.job_events(&job_id).await;
            let body = if stream {
                Some(responder.begin_stream(200, "text/event-stream")?)
            } else {
                None
            };
            let drain = async {
                while let Some(event) = events.recv().await {
                    match event {
                        JobEvent::Done { result } => return Ok(result),
                        JobEvent::Failed { message, rejected } => {
                            return Err(if rejected {
                                bad_request(message)
                            } else {
                                server_error(message)
                            });
                        }
                        JobEvent::Cancelled => {
                            return Err(server_error("generation was cancelled"));
                        }
                        other => {
                            if let Some(body) = &body {
                                send_progress(body, &other);
                            }
                        }
                    }
                }
                Err(server_error("the job ended without saying how"))
            };
            let finished = tokio::select! {
                finished = tokio::time::timeout(RUN_TIMEOUT, drain) => finished,
                _ = responder.closed() => {
                    return Err(GatewayError::new(
                        GatewayErrorKind::ClientClosed,
                        "the client went away; the job was cancelled",
                    ));
                }
            };
            let outcome = finished.unwrap_or_else(|_| {
                Err(GatewayError::new(
                    GatewayErrorKind::Timeout,
                    format!(
                        "audio generation ran longer than {}s",
                        RUN_TIMEOUT.as_secs()
                    ),
                ))
            });
            if outcome.is_ok() {
                guard.armed = false;
            }
            let audio = match outcome {
                Ok(artifacts) => read_artifact(port, artifacts.first())
                    .await
                    .and_then(|bytes| {
                        bytes.ok_or_else(|| {
                            server_error(format!("{} produced no audio", record.name))
                        })
                    }),
                Err(error) => Err(error),
            };

            match (body, audio) {
                (Some(body), Ok(bytes)) => {
                    send_event(
                        &body,
                        "complete",
                        &json!({
                            "created": now_unix_seconds(),
                            "model": record.wire_id(),
                            "format": format,
                            "b64_json": BASE64_STANDARD.encode(&bytes),
                        }),
                    );
                    body.end();
                }
                // The head is out, so the failure is told as the stream's last
                // event; returning it still records it as the request's outcome.
                (Some(body), Err(error)) => {
                    send_event(&body, "error", &json!({ "message": error.to_string() }));
                    body.end();
                    return Err(error);
                }
                (None, Ok(bytes)) => {
                    responder.respond(200, content_type(&format), bytes, Vec::new());
                }
                (None, Err(error)) => return Err(error),
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::audio()),
            ))
        })
    }
}

/// Cancels a job when dropped while still armed: the request that started it
/// ended before the job did.
struct JobGuard<'a> {
    port: &'a dyn GatewayPort,
    job_id: &'a str,
    armed: bool,
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.port.abandon(self.job_id);
        }
    }
}

/// The bytes of the first artifact a job made, `None` when it made none or
/// the file is gone.
async fn read_artifact(
    port: &dyn GatewayPort,
    id: Option<&String>,
) -> Result<Option<Vec<u8>>, GatewayError> {
    match id {
        Some(id) => port
            .artifact_data(id)
            .await
            .map_err(|error| server_error(error.to_string())),
        None => Ok(None),
    }
}

/// The request's fields as the audio contract names them: mlx-serve's
/// `duration_seconds` is `duration`, OpenAI's `response_format` is `format`,
/// and the routing keys (`model`, `stream`) are left out.
fn request_fields(
    body: &BTreeMap<String, JsonValue>,
) -> Result<BTreeMap<String, JsonValue>, GatewayError> {
    let mut fields = BTreeMap::new();
    for (key, value) in body {
        let named = match key.as_str() {
            "model" | "stream" => continue,
            "duration_seconds" => "duration",
            "response_format" => "format",
            other => other,
        };
        if fields.insert(named.to_owned(), value.clone()).is_some() {
            return Err(bad_request(format!(
                "{named} is given twice; send one of {key} or {named}"
            )));
        }
    }
    Ok(fields)
}

/// A `400` for a model that does not make audio, pointing a voice at the
/// speech route.
fn refuse_unless_generator(record: &ModelRecord) -> Result<(), GatewayError> {
    if record.can(&Capability::audio()) {
        return Ok(());
    }
    if record.can(&Capability::speak()) {
        return Err(bad_request(format!(
            "{} reads text aloud rather than making audio from a description; \
             use POST /v1/audio/speech",
            record.display_name()
        )));
    }
    Err(bad_request(format!(
        "{} does not make audio",
        record.display_name()
    )))
}

fn content_type(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        _ => "audio/flac",
    }
}

/// A job event as a `progress` server-sent event, when it says anything.
fn send_progress(body: &GatewayStreamBody, event: &JobEvent) {
    let data = match event {
        JobEvent::Queued { reason } => json!({ "stage": reason.as_deref().unwrap_or("queued") }),
        JobEvent::Preparing => json!({ "stage": "preparing" }),
        JobEvent::Running => json!({ "stage": "running" }),
        JobEvent::Status(message) => json!({ "stage": message }),
        JobEvent::Progress(progress) => json!({
            "step": progress.step,
            "total": progress.total_steps,
            "fraction": progress.fraction,
        }),
        _ => return,
    };
    send_event(body, "progress", &data);
}

fn send_event(body: &GatewayStreamBody, name: &str, data: &serde_json::Value) {
    body.write(format!("event: {name}\ndata: {data}\n\n").into_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(pairs: &[(&str, JsonValue)]) -> BTreeMap<String, JsonValue> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn mlx_serve_names_become_the_contracts() {
        let fields = request_fields(&object(&[
            ("model", JsonValue::String("m".into())),
            ("stream", JsonValue::Bool(true)),
            ("duration_seconds", JsonValue::Int(30)),
            ("response_format", JsonValue::String("wav".into())),
            ("prompt", JsonValue::String("p".into())),
        ]))
        .unwrap();
        assert_eq!(
            fields,
            object(&[
                ("duration", JsonValue::Int(30)),
                ("format", JsonValue::String("wav".into())),
                ("prompt", JsonValue::String("p".into())),
            ])
        );
    }

    #[test]
    fn one_field_under_two_names_is_refused() {
        let error = request_fields(&object(&[
            ("duration", JsonValue::Int(10)),
            ("duration_seconds", JsonValue::Int(30)),
        ]))
        .expect_err("refused");
        assert!(error.to_string().contains("given twice"), "{error}");
    }
}
