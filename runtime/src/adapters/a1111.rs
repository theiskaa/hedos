//! The AUTOMATIC1111 daemon adapter: generates images through a running
//! AUTOMATIC1111 `txt2img` API over HTTP. A [`JobRunning`] adapter — streaming
//! `invoke` is rejected; work happens through `run`.

use std::collections::BTreeMap;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};
use serde_json::{Value, json};

use super::daemon_liveness::{
    Daemon, DaemonError, DaemonLiveness, daemon_error, dimensions, run_daemon_job,
};
use super::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream};
use crate::util::base64_decode;

/// The AUTOMATIC1111 daemon adapter.
pub struct A1111Adapter {
    id: RuntimeId,
    liveness: DaemonLiveness,
    client: reqwest::Client,
}

impl A1111Adapter {
    /// An adapter serving the AUTOMATIC1111 instance tracked by `liveness`.
    pub fn new(liveness: DaemonLiveness) -> Self {
        Self {
            id: RuntimeId::a1111(),
            liveness,
            client: reqwest::Client::new(),
        }
    }

    /// The `txt2img` request body for a payload.
    fn request_body(payload: &JsonValue) -> Value {
        let empty = BTreeMap::new();
        let object = payload.as_object().unwrap_or(&empty);
        let prompt = object
            .get("prompt")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let steps = object
            .get("steps")
            .and_then(JsonValue::as_i64)
            .unwrap_or(20);
        let cfg = object
            .get("guidance")
            .and_then(JsonValue::as_f64)
            .or_else(|| object.get("cfg_scale").and_then(JsonValue::as_f64))
            .unwrap_or(7.0);
        let (width, height) = dimensions(object, 512);
        let seed = object.get("seed").and_then(JsonValue::as_i64).unwrap_or(-1);
        json!({
            "prompt": prompt,
            "steps": steps,
            "cfg_scale": cfg,
            "width": width,
            "height": height,
            "seed": seed,
        })
    }
}

impl RuntimeAdapter for A1111Adapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id) && capability == &Capability::image()
    }

    fn bid(&self, record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::Diffusers
            || !identified.capabilities.contains(&Capability::image())
        {
            return None;
        }
        let state = self.liveness.current().a1111;
        if state.alive && DaemonLiveness::matches(record, &state.models) {
            Some(RuntimeBid::new(RunTier::Native, BidPreference::A1111))
        } else {
            None
        }
    }

    fn invoke(
        &self,
        _record: &ModelRecord,
        _capability: Capability,
        _payload: JsonValue,
    ) -> ChunkStream {
        RuntimeStream::failed(RuntimeError::WrongExecutionMode)
    }
}

impl JobRunning for A1111Adapter {
    fn run(&self, _record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        let liveness = self.liveness.clone();
        let client = self.client.clone();
        run_daemon_job(
            Daemon::A1111,
            self.liveness.clone(),
            Box::pin(async move {
                let body = Self::request_body(&payload);
                let response = client
                    .post(format!("{}/sdapi/v1/txt2img", liveness.a1111_url()))
                    .json(&body)
                    .send()
                    .await
                    .map_err(daemon_error)?;
                let object: Value = response.json().await.map_err(daemon_error)?;
                let encoded = object
                    .get("images")
                    .and_then(Value::as_array)
                    .and_then(|images| images.first())
                    .and_then(Value::as_str)
                    .ok_or_else(|| DaemonError::Failed("A1111 returned no image".to_owned()))?;
                base64_decode(encoded).ok_or_else(|| {
                    DaemonError::Failed("A1111 returned malformed base64".to_owned())
                })
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    fn adapter() -> A1111Adapter {
        A1111Adapter::new(DaemonLiveness::new(
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        ))
    }

    fn record() -> ModelRecord {
        let mut record = ModelRecord::new(
            "sd-model",
            Modality::image(),
            Vec::new(),
            ModelSource::new(
                SourceKind::huggingface_cache(),
                "/models/sd-model.safetensors",
            ),
        );
        record.runtime.id = Some(RuntimeId::a1111());
        record.state = ModelState::Ready;
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(Modality::image()), caps, ExecutionMode::Job)
    }

    #[test]
    fn it_serves_only_image_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::a1111());
        assert!(adapter.can_serve(&record(), &Capability::image()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_does_not_bid_when_the_daemon_is_not_serving_the_model() {
        let adapter = adapter();
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Diffusers, vec![Capability::image()])
                )
                .is_none()
        );
    }

    #[test]
    fn it_does_not_bid_without_the_diffusers_format_or_image_capability() {
        let adapter = adapter();
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Safetensors, vec![Capability::image()])
                )
                .is_none()
        );
        assert!(
            adapter
                .bid(
                    &record(),
                    &identified(ModelFormat::Diffusers, vec![Capability::chat()])
                )
                .is_none()
        );
    }

    #[test]
    fn the_request_body_encodes_the_payload_with_a1111_defaults() {
        let mut object = BTreeMap::new();
        object.insert("prompt".to_owned(), JsonValue::String("a dog".to_owned()));
        object.insert("size".to_owned(), JsonValue::String("640x480".to_owned()));
        let body = A1111Adapter::request_body(&JsonValue::Object(object));
        assert_eq!(body["prompt"], "a dog");
        assert_eq!(body["steps"], 20);
        assert_eq!(body["cfg_scale"], 7.0);
        assert_eq!(body["width"], 640);
        assert_eq!(body["height"], 480);
        // A1111's default seed is -1 (random), unlike ComfyUI's 0.
        assert_eq!(body["seed"], -1);
    }

    #[tokio::test]
    async fn streaming_invoke_is_rejected_as_wrong_execution_mode() {
        let adapter = adapter();
        let mut stream = adapter.invoke(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::WrongExecutionMode))
        ));
    }

    #[tokio::test]
    async fn run_against_a_dead_daemon_starts_then_fails() {
        let adapter = adapter();
        let mut stream = adapter.run(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        assert!(matches!(
            stream.recv().await,
            Some(Ok(kernel::jobs::JobRuntimeEvent::Started))
        ));
        assert!(matches!(
            stream.recv().await,
            Some(Err(RuntimeError::Failed(_)))
        ));
    }

    #[tokio::test]
    async fn a_transport_failure_marks_the_daemon_dead() {
        use super::super::daemon_liveness::{DaemonState, Snapshot};
        let liveness = DaemonLiveness::new("http://127.0.0.1:1", "http://127.0.0.1:1");
        liveness.seed(Snapshot {
            a1111: DaemonState {
                alive: true,
                models: vec!["sd-model".to_owned()],
            },
            ..Default::default()
        });
        let adapter = A1111Adapter::new(liveness.clone());
        let mut stream = adapter.run(
            &record(),
            Capability::image(),
            JsonValue::Object(Default::default()),
        );
        while let Some(item) = stream.recv().await {
            if matches!(item, Err(RuntimeError::Failed(_))) {
                break;
            }
        }
        assert!(!liveness.current().a1111.alive);
    }
}
