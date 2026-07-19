//! The OpenAI speech handler: `/v1/audio/speech`. Invokes the `speak`
//! capability, accumulates the streamed audio frames, and returns one WAV file.

use std::collections::BTreeMap;
use std::time::Duration;

use kernel::capabilities::CapabilityChunk;
use kernel::records::{Capability, JsonValue};
use runtime::sidecar::DEFAULT_SAMPLE_RATE;

use super::{
    GatewayHandling, HandlerFuture, bad_request, required_model, runtime_failed, server_error,
};
use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;

/// A single utterance is bounded before its stream is abandoned.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// `POST /v1/audio/speech`.
pub struct OpenAISpeechHandler;

impl GatewayHandling for OpenAISpeechHandler {
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
            let input = body
                .get("input")
                .and_then(JsonValue::as_str)
                .filter(|input| !input.is_empty())
                .ok_or_else(|| bad_request("input is required"))?;
            if matches!(body.get("response_format"), Some(JsonValue::String(format)) if format != "wav")
            {
                return Err(bad_request(
                    "only wav output is available — set response_format to wav",
                ));
            }

            let record = resolve_authorized(
                port,
                model,
                Capability::speak(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;

            let mut voice = body
                .get("voice")
                .and_then(JsonValue::as_str)
                .filter(|voice| !voice.is_empty())
                .map(str::to_owned);
            if voice.is_none() {
                voice = port.voices(&record.id).await?.into_iter().next();
            }
            let mut payload = BTreeMap::new();
            payload.insert("text".to_owned(), JsonValue::String(input.to_owned()));
            if let Some(voice) = voice {
                payload.insert("voice".to_owned(), JsonValue::String(voice));
            }
            if let Some(speed) = body.get("speed").and_then(JsonValue::as_f64) {
                payload.insert("speed".to_owned(), JsonValue::Double(speed));
            }

            let mut stream = port
                .invoke(&record.id, Capability::speak(), JsonValue::Object(payload))
                .await?;
            let drain = async {
                let mut pcm = Vec::new();
                let mut sample_rate = DEFAULT_SAMPLE_RATE;
                while let Some(result) = stream.recv().await {
                    if let CapabilityChunk::Audio(frame) = result.map_err(runtime_failed)? {
                        if pcm.is_empty() {
                            sample_rate = frame.sample_rate;
                        }
                        pcm.extend_from_slice(&frame.data);
                    }
                }
                Ok::<_, GatewayError>((pcm, sample_rate))
            };
            let (pcm, sample_rate) = match tokio::time::timeout(RUN_TIMEOUT, drain).await {
                Ok(result) => result?,
                Err(_) => {
                    // `Timeout` already defaults its wire code to "timeout".
                    return Err(GatewayError::new(
                        GatewayErrorKind::Timeout,
                        format!("speech timed out after {}s", RUN_TIMEOUT.as_secs()),
                    ));
                }
            };
            if pcm.is_empty() {
                return Err(server_error(format!("{} produced no audio", record.name)));
            }
            let rate = u32::try_from(sample_rate).unwrap_or(DEFAULT_SAMPLE_RATE as u32);
            responder.respond(
                200,
                "audio/wav",
                runtime::audio::wav_from_pcm(&pcm, rate),
                Vec::new(),
            );
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::speak()),
            ))
        })
    }
}
