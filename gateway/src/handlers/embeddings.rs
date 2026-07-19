//! The embeddings handlers: OpenAI `/v1/embeddings` and Ollama `/api/embed`
//! (plus the legacy `/api/embeddings`). Both resolve an embedding model, invoke
//! it once, collect the vectors, and shape them into their dialect's response.

use std::collections::BTreeMap;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use kernel::capabilities::{CapabilityChunk, GenerationStats};
use kernel::records::{Capability, JsonValue, ModelRecord};
use runtime::facade::KernelError;
use serde_json::{Value, json};

use super::{GatewayHandling, HandlerFuture, bad_request, respond_json, runtime_failed};
use crate::admission::GatewayWorkKind;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::resolver::resolve_authorized;
use crate::responder::GatewayResponder;

/// Invoke `record`'s embedding runtime over `inputs` and collect one vector per
/// input, plus the terminal stats. Maps an unsupported-capability failure to a
/// clear 501 rather than a generic 400.
async fn collect_embeddings(
    port: &dyn GatewayPort,
    record: &ModelRecord,
    inputs: Vec<String>,
) -> Result<(Vec<Vec<f64>>, Option<GenerationStats>), GatewayError> {
    let count = inputs.len();
    let input = if count == 1 {
        JsonValue::String(inputs.into_iter().next().unwrap_or_default())
    } else {
        JsonValue::Array(inputs.into_iter().map(JsonValue::String).collect())
    };
    let mut payload = BTreeMap::new();
    payload.insert("input".to_owned(), input);

    let mut stream = match port
        .invoke(&record.id, Capability::embed(), JsonValue::Object(payload))
        .await
    {
        Ok(stream) => stream,
        Err(KernelError::CapabilityUnsupported { .. }) => {
            return Err(GatewayError::new(
                GatewayErrorKind::NotSupported,
                format!("{} has no embeddings runtime on this machine", record.name),
            )
            .with_code("capability_unsupported"));
        }
        Err(other) => return Err(other.into()),
    };

    let mut vectors = Vec::new();
    let mut final_stats = None;
    while let Some(result) = stream.recv().await {
        match result.map_err(runtime_failed)? {
            CapabilityChunk::Vector(vector) => vectors.push(vector),
            CapabilityChunk::Done(stats) => final_stats = stats,
            _ => {}
        }
    }
    if vectors.is_empty() {
        return Err(GatewayError::new(
            GatewayErrorKind::ServerError,
            format!("{} produced no embeddings", record.name),
        ));
    }
    if vectors.len() != count {
        return Err(GatewayError::new(
            GatewayErrorKind::ServerError,
            format!(
                "{} returned {} embeddings for {} inputs",
                record.name,
                vectors.len(),
                count
            ),
        ));
    }
    Ok((vectors, final_stats))
}

/// The little-endian f32 bytes of a vector, base64-encoded (the OpenAI `base64`
/// encoding format).
fn vector_base64(vector: &[f64]) -> String {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for &value in vector {
        bytes.extend_from_slice(&(value as f32).to_le_bytes());
    }
    BASE64_STANDARD.encode(&bytes)
}

fn required_model(body: &BTreeMap<String, JsonValue>) -> Result<&str, GatewayError> {
    body.get("model")
        .and_then(JsonValue::as_str)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| bad_request("model is required"))
}

fn string_list(items: &[JsonValue]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|value| value.as_str().map(str::to_owned))
        .collect()
}

/// Whether `items` is an array of integers or of integer arrays — the tokenized
/// input OpenAI accepts but no local runtime does.
fn is_token_array(items: &[JsonValue]) -> bool {
    let all_ints = items.iter().all(|value| matches!(value, JsonValue::Int(_)));
    let all_int_arrays = items.iter().all(|value| {
        matches!(value, JsonValue::Array(inner) if inner.iter().all(|token| matches!(token, JsonValue::Int(_))))
    });
    all_ints || all_int_arrays
}

/// `POST /v1/embeddings`.
pub struct OpenAIEmbeddingsHandler;

impl OpenAIEmbeddingsHandler {
    fn inputs(body: &BTreeMap<String, JsonValue>) -> Result<Vec<String>, GatewayError> {
        match body.get("input") {
            Some(JsonValue::String(single)) if !single.is_empty() => Ok(vec![single.clone()]),
            Some(JsonValue::Array(items)) if !items.is_empty() => match string_list(items) {
                Some(strings) => Ok(strings),
                None if is_token_array(items) => Err(bad_request(
                    "token array input is not supported — send text",
                )),
                None => Err(bad_request("input is required")),
            },
            _ => Err(bad_request("input is required")),
        }
    }
}

impl GatewayHandling for OpenAIEmbeddingsHandler {
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
            let inputs = Self::inputs(&body)?;
            let format = body
                .get("encoding_format")
                .and_then(JsonValue::as_str)
                .unwrap_or("float");
            if format != "float" && format != "base64" {
                return Err(
                    bad_request(format!("encoding_format '{format}' is not supported"))
                        .with_code("unsupported_parameter"),
                );
            }
            if body.contains_key("dimensions") {
                return Err(bad_request(
                    "dimensions is not supported — no local runtime truncates embeddings",
                )
                .with_code("unsupported_parameter"));
            }
            let record = resolve_authorized(
                port,
                model,
                Capability::embed(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;
            let (vectors, stats) = collect_embeddings(port, &record, inputs).await?;

            let data: Vec<Value> = vectors
                .iter()
                .enumerate()
                .map(|(index, vector)| {
                    let embedding = if format == "base64" {
                        Value::String(vector_base64(vector))
                    } else {
                        json!(vector)
                    };
                    json!({ "object": "embedding", "embedding": embedding, "index": index })
                })
                .collect();
            let prompt_tokens = stats.and_then(|stats| stats.prompt_tokens).unwrap_or(0);
            respond_json(
                responder,
                &json!({
                    "object": "list",
                    "data": data,
                    "model": model,
                    "usage": { "prompt_tokens": prompt_tokens, "total_tokens": prompt_tokens },
                }),
            );
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::embed()),
            ))
        })
    }
}

/// `POST /api/embed` and the legacy `POST /api/embeddings`.
pub struct OllamaEmbedHandler;

impl OllamaEmbedHandler {
    fn inputs(
        body: &BTreeMap<String, JsonValue>,
        legacy: bool,
    ) -> Result<Vec<String>, GatewayError> {
        let key = if legacy { "prompt" } else { "input" };
        match body.get(key) {
            Some(JsonValue::String(single)) if !single.is_empty() => Ok(vec![single.clone()]),
            Some(JsonValue::Array(items)) if !legacy && !items.is_empty() => {
                string_list(items).ok_or_else(|| bad_request(format!("{key} is required")))
            }
            _ => Err(bad_request(format!("{key} is required"))),
        }
    }
}

impl GatewayHandling for OllamaEmbedHandler {
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
            for unsupported in ["truncate", "dimensions"] {
                if body.contains_key(unsupported) {
                    return Err(bad_request(format!(
                        "the parameter '{unsupported}' is not supported"
                    ))
                    .with_code("unsupported_parameter"));
                }
            }
            let legacy = request.path == "/api/embeddings";
            let inputs = Self::inputs(&body, legacy)?;
            let record = resolve_authorized(
                port,
                model,
                Capability::embed(),
                GatewayWorkKind::Stream,
                identity,
            )
            .await?;
            let (vectors, stats) = collect_embeddings(port, &record, inputs).await?;

            if legacy {
                respond_json(responder, &json!({ "embedding": vectors[0] }));
            } else {
                let mut object = json!({ "model": model, "embeddings": vectors });
                if let Some(prompt_tokens) = stats.and_then(|stats| stats.prompt_tokens) {
                    object["prompt_eval_count"] = json!(prompt_tokens);
                }
                respond_json(responder, &object);
            }
            Ok(GatewayOutcome::ok_for(
                Some(&record.id),
                Some(&Capability::embed()),
            ))
        })
    }
}
