//! The ComfyUI daemon adapter: generates images by submitting a fixed
//! text-to-image graph to a running ComfyUI instance over HTTP and polling its
//! history for the result. A [`JobRunning`] adapter — streaming `invoke` is
//! rejected; work happens through `run`.

use std::collections::BTreeMap;
use std::time::Duration;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{IdentifiedModel, ModelFormat, RuntimeBid};
use serde_json::{Value, json};

use super::daemon_liveness::{
    Daemon, DaemonError, DaemonLiveness, daemon_error, dimensions, run_daemon_job,
};
use super::{ChunkStream, JobRunning, JobStream, RuntimeAdapter, RuntimeError, RuntimeStream};

/// How often to poll ComfyUI's history endpoint while a generation runs.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The ComfyUI daemon adapter.
pub struct ComfyUiAdapter {
    id: RuntimeId,
    liveness: DaemonLiveness,
    client: reqwest::Client,
}

impl ComfyUiAdapter {
    /// An adapter serving the ComfyUI instance tracked by `liveness`.
    pub fn new(liveness: DaemonLiveness) -> Self {
        Self {
            id: RuntimeId::comfy_ui(),
            liveness,
            client: reqwest::Client::new(),
        }
    }

    /// The checkpoint name to load for `record`: its first match among the
    /// daemon's `served_models`, else the record's own name.
    fn checkpoint_name(record: &ModelRecord, served_models: &[String]) -> String {
        DaemonLiveness::matching_models(record, served_models)
            .into_iter()
            .next()
            .unwrap_or_else(|| record.name.clone())
    }

    /// The ComfyUI prompt graph for a text-to-image request: a standard
    /// checkpoint → sampler → VAE-decode → save pipeline parameterized by the
    /// payload and the chosen `checkpoint`.
    fn graph(payload: &JsonValue, checkpoint: &str) -> Value {
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
        let seed = object.get("seed").and_then(JsonValue::as_i64).unwrap_or(0);
        json!({
            "3": {
                "class_type": "KSampler",
                "inputs": {
                    "seed": seed, "steps": steps, "cfg": cfg, "sampler_name": "euler",
                    "scheduler": "normal", "denoise": 1.0,
                    "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0],
                    "latent_image": ["5", 0],
                },
            },
            "4": {
                "class_type": "CheckpointLoaderSimple",
                "inputs": { "ckpt_name": checkpoint },
            },
            "5": {
                "class_type": "EmptyLatentImage",
                "inputs": { "width": width, "height": height, "batch_size": 1 },
            },
            "6": {
                "class_type": "CLIPTextEncode",
                "inputs": { "text": prompt, "clip": ["4", 1] },
            },
            "7": {
                "class_type": "CLIPTextEncode",
                "inputs": { "text": "", "clip": ["4", 1] },
            },
            "8": {
                "class_type": "VAEDecode",
                "inputs": { "samples": ["3", 0], "vae": ["4", 2] },
            },
            "9": {
                "class_type": "SaveImage",
                "inputs": { "filename_prefix": "hedos", "images": ["8", 0] },
            },
        })
    }
}

impl RuntimeAdapter for ComfyUiAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(self.id()) && capability == &Capability::image()
    }

    fn bid(&self, record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        if identified.format != ModelFormat::Diffusers
            || !identified.capabilities.contains(&Capability::image())
        {
            return None;
        }
        let state = self.liveness.current().comfy_ui;
        if state.alive && DaemonLiveness::matches(record, &state.models) {
            Some(RuntimeBid::new(RunTier::Native, BidPreference::COMFY_UI))
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

impl JobRunning for ComfyUiAdapter {
    fn run(&self, record: &ModelRecord, _capability: Capability, payload: JsonValue) -> JobStream {
        let liveness = self.liveness.clone();
        let client = self.client.clone();
        let record = record.clone();
        run_daemon_job(
            Daemon::ComfyUi,
            self.liveness.clone(),
            Box::pin(async move {
                let served = liveness.current().comfy_ui.models;
                let checkpoint = Self::checkpoint_name(&record, &served);
                let graph = Self::graph(&payload, &checkpoint);
                let base = liveness.comfy_url();
                let prompt_id = submit(&client, base, &graph).await?;
                await_result(&client, base, &prompt_id).await
            }),
        )
    }
}

/// Submit `graph` to ComfyUI's `/prompt` endpoint and return the prompt id.
async fn submit(
    client: &reqwest::Client,
    base: &str,
    graph: &Value,
) -> Result<String, DaemonError> {
    let response = client
        .post(format!("{base}/prompt"))
        .json(&json!({ "prompt": graph }))
        .send()
        .await
        .map_err(daemon_error)?;
    let object: Value = response.json().await.map_err(daemon_error)?;
    object
        .get("prompt_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| DaemonError::Failed("ComfyUI did not return a prompt id".to_owned()))
}

/// Poll `/history/{prompt_id}` until the generation's outputs appear, then fetch
/// and return the first image's bytes.
async fn await_result(
    client: &reqwest::Client,
    base: &str,
    prompt_id: &str,
) -> Result<Vec<u8>, DaemonError> {
    let history_url = format!("{base}/history/{prompt_id}");
    loop {
        let response = client
            .get(&history_url)
            .send()
            .await
            .map_err(daemon_error)?;
        let object: Value = response.json().await.map_err(daemon_error)?;
        if let Some(outputs) = object.get(prompt_id).and_then(|entry| entry.get("outputs")) {
            return match first_image(client, base, outputs).await? {
                Some(image) => Ok(image),
                None => Err(DaemonError::Failed("ComfyUI produced no image".to_owned())),
            };
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Fetch the first saved image referenced in `outputs` through ComfyUI's `/view`
/// endpoint. `Ok(None)` if no output node carries an image.
async fn first_image(
    client: &reqwest::Client,
    base: &str,
    outputs: &Value,
) -> Result<Option<Vec<u8>>, DaemonError> {
    let Some(map) = outputs.as_object() else {
        return Ok(None);
    };
    for node in map.values() {
        let Some(first) = node
            .get("images")
            .and_then(Value::as_array)
            .and_then(|images| images.first())
        else {
            continue;
        };
        let Some(filename) = first.get("filename").and_then(Value::as_str) else {
            continue;
        };
        let subfolder = first.get("subfolder").and_then(Value::as_str).unwrap_or("");
        let kind = first
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("output");
        let response = client
            .get(format!("{base}/view"))
            .query(&[
                ("filename", filename),
                ("subfolder", subfolder),
                ("type", kind),
            ])
            .send()
            .await
            .map_err(daemon_error)?;
        let bytes = response.bytes().await.map_err(daemon_error)?;
        return Ok(Some(bytes.to_vec()));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{ExecutionMode, Modality, ModelSource, ModelState, SourceKind};

    fn adapter() -> ComfyUiAdapter {
        ComfyUiAdapter::new(DaemonLiveness::new(
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
        record.runtime.id = Some(RuntimeId::comfy_ui());
        record.state = ModelState::Ready;
        record
    }

    fn identified(format: ModelFormat, caps: Vec<Capability>) -> IdentifiedModel {
        IdentifiedModel::new(format, Some(Modality::image()), caps, ExecutionMode::Job)
    }

    #[test]
    fn it_serves_only_image_for_its_own_runtime() {
        let adapter = adapter();
        assert_eq!(adapter.id(), &RuntimeId::comfy_ui());
        assert!(adapter.can_serve(&record(), &Capability::image()));
        assert!(!adapter.can_serve(&record(), &Capability::chat()));
    }

    #[test]
    fn it_does_not_bid_when_the_daemon_is_not_serving_the_model() {
        // Liveness is empty (no probe ran), so no match → no bid.
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
    fn the_graph_encodes_the_payload_and_the_checkpoint() {
        let mut object = std::collections::BTreeMap::new();
        object.insert("prompt".to_owned(), JsonValue::String("a cat".to_owned()));
        object.insert("steps".to_owned(), JsonValue::Int(30));
        object.insert("guidance".to_owned(), JsonValue::Double(8.5));
        object.insert("seed".to_owned(), JsonValue::Int(42));
        object.insert("width".to_owned(), JsonValue::Int(768));
        object.insert("height".to_owned(), JsonValue::Int(768));
        let graph = ComfyUiAdapter::graph(&JsonValue::Object(object), "model.safetensors");

        assert_eq!(graph["4"]["inputs"]["ckpt_name"], "model.safetensors");
        assert_eq!(graph["3"]["inputs"]["steps"], 30);
        assert_eq!(graph["3"]["inputs"]["cfg"], 8.5);
        assert_eq!(graph["3"]["inputs"]["seed"], 42);
        assert_eq!(graph["5"]["inputs"]["width"], 768);
        assert_eq!(graph["6"]["inputs"]["text"], "a cat");
        assert_eq!(graph["7"]["inputs"]["text"], "");

        // Node classes, wiring refs, and fixed constants.
        assert_eq!(graph["3"]["class_type"], "KSampler");
        assert_eq!(graph["3"]["inputs"]["sampler_name"], "euler");
        assert_eq!(graph["3"]["inputs"]["scheduler"], "normal");
        assert_eq!(graph["3"]["inputs"]["denoise"], 1.0);
        assert_eq!(graph["3"]["inputs"]["model"], json!(["4", 0]));
        assert_eq!(graph["3"]["inputs"]["positive"], json!(["6", 0]));
        assert_eq!(graph["3"]["inputs"]["negative"], json!(["7", 0]));
        assert_eq!(graph["3"]["inputs"]["latent_image"], json!(["5", 0]));
        assert_eq!(graph["5"]["inputs"]["batch_size"], 1);
        assert_eq!(graph["6"]["inputs"]["clip"], json!(["4", 1]));
        assert_eq!(graph["8"]["class_type"], "VAEDecode");
        assert_eq!(graph["8"]["inputs"]["samples"], json!(["3", 0]));
        assert_eq!(graph["8"]["inputs"]["vae"], json!(["4", 2]));
        assert_eq!(graph["9"]["class_type"], "SaveImage");
        assert_eq!(graph["9"]["inputs"]["filename_prefix"], "hedos");
        assert_eq!(graph["9"]["inputs"]["images"], json!(["8", 0]));
    }

    #[test]
    fn the_graph_falls_back_to_cfg_scale_and_defaults() {
        let mut object = std::collections::BTreeMap::new();
        object.insert("cfg_scale".to_owned(), JsonValue::Double(6.0));
        let graph = ComfyUiAdapter::graph(&JsonValue::Object(object), "m");
        assert_eq!(graph["3"]["inputs"]["cfg"], 6.0);
        assert_eq!(graph["3"]["inputs"]["steps"], 20);
        assert_eq!(graph["5"]["inputs"]["width"], 512);
        // ComfyUI's default seed is 0 (unlike A1111's -1).
        assert_eq!(graph["3"]["inputs"]["seed"], 0);
    }

    #[test]
    fn checkpoint_name_falls_back_to_the_record_name() {
        assert_eq!(
            ComfyUiAdapter::checkpoint_name(&record(), &[]),
            "sd-model".to_owned()
        );
        assert_eq!(
            ComfyUiAdapter::checkpoint_name(&record(), &["sd-model.safetensors".to_owned()]),
            "sd-model.safetensors".to_owned()
        );
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
            comfy_ui: DaemonState {
                alive: true,
                models: vec!["sd-model".to_owned()],
            },
            ..Default::default()
        });
        let adapter = ComfyUiAdapter::new(liveness.clone());
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
        // The dead-port connection error is a transport failure → mark dead.
        assert!(!liveness.current().comfy_ui.alive);
    }
}
