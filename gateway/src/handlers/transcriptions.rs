//! The OpenAI transcription handler: `/v1/audio/transcriptions`. Reads a
//! multipart upload, decodes the WAV to PCM, invokes the `transcribe`
//! capability, and returns the text as JSON or plain text.

use std::collections::BTreeMap;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use serde_json::json;

use super::{GatewayHandling, HandlerFuture, bad_request, respond_json, runtime_failed};
use crate::admission::GatewayWorkKind;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;
use crate::wire::multipart::{self, Part};

/// Parameters accepted by the OpenAI API that this transcriber does not honor;
/// sending any of them is rejected rather than silently ignored.
const UNSUPPORTED_FIELDS: &[&str] = &[
    "language",
    "prompt",
    "temperature",
    "timestamp_granularities",
];

/// `POST /v1/audio/transcriptions`.
pub struct OpenAITranscriptionsHandler;

impl GatewayHandling for OpenAITranscriptionsHandler {
    fn handle<'a>(
        &'a self,
        request: &'a GatewayRequest,
        identity: &'a GatewayIdentity,
        port: &'a dyn GatewayPort,
        responder: &'a GatewayResponder,
    ) -> HandlerFuture<'a> {
        Box::pin(async move {
            let boundary = multipart::boundary(request.header("Content-Type"))
                .ok_or_else(|| bad_request("transcriptions require multipart/form-data"))?;
            let parts = multipart::parse(&request.body, &boundary);

            let model = text_field(&parts, "model")
                .filter(|model| !model.is_empty())
                .ok_or_else(|| bad_request("model is required"))?;
            let file = field(&parts, "file").ok_or_else(|| bad_request("file is required"))?;
            let response_format = text_field(&parts, "response_format").unwrap_or("json");
            if response_format != "json" && response_format != "text" {
                return Err(bad_request(format!(
                    "response_format '{response_format}' is not supported"
                ))
                .with_code("unsupported_parameter"));
            }
            for unsupported in UNSUPPORTED_FIELDS {
                if field(&parts, unsupported).is_some() {
                    return Err(bad_request(format!(
                        "the parameter '{unsupported}' is not supported yet"
                    ))
                    .with_code("unsupported_parameter"));
                }
            }

            let record = resolve_authorized(
                port,
                model,
                Capability::transcribe(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;

            let (pcm, sample_rate) = runtime::audio::pcm_from_wav(&file.data)
                .ok_or_else(|| bad_request("audio is not a RIFF WAVE file"))?;
            let payload = json_payload(&pcm, sample_rate);

            let mut stream = port
                .invoke(&record.id, Capability::transcribe(), payload)
                .await?;
            let mut transcript = String::new();
            while let Some(result) = stream.recv().await {
                match result.map_err(runtime_failed)? {
                    CapabilityChunk::Text(text) | CapabilityChunk::Segment { text, .. } => {
                        transcript.push_str(&text);
                    }
                    _ => {}
                }
            }

            if response_format == "text" {
                responder.respond(
                    200,
                    "text/plain; charset=utf-8",
                    transcript.into_bytes(),
                    Vec::new(),
                );
            } else {
                respond_json(responder, &json!({ "text": transcript }));
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::transcribe()),
            ))
        })
    }
}

/// The first part named `name`, if any.
fn field<'a>(parts: &'a [Part], name: &str) -> Option<&'a Part> {
    parts.iter().find(|part| part.name.as_deref() == Some(name))
}

/// The UTF-8 text of the first part named `name`, if it decodes.
fn text_field<'a>(parts: &'a [Part], name: &str) -> Option<&'a str> {
    field(parts, name).and_then(|part| std::str::from_utf8(&part.data).ok())
}

/// The `transcribe` invoke payload: base64 PCM plus its sample rate.
fn json_payload(pcm: &[u8], sample_rate: u32) -> JsonValue {
    let mut fields = BTreeMap::new();
    fields.insert(
        "pcm".to_owned(),
        JsonValue::String(BASE64_STANDARD.encode(pcm)),
    );
    fields.insert(
        "sampleRate".to_owned(),
        JsonValue::Int(i64::from(sample_rate)),
    );
    JsonValue::Object(fields)
}
