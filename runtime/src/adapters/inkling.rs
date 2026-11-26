//! Reading an Inkling checkpoint's config for what the pinned mlx-vlm loads:
//! MLX conversions of any Inkling, and the native Hugging Face snapshots of
//! Inkling 2 (Mini and Small) in place, their mixed-precision weights included.

use std::path::Path;

use kernel::records::ModelRecord;
use serde_json::Value;

use crate::sidecar::SidecarModelPaths;

/// The model types whose original checkpoints mlx-vlm loads without
/// conversion: Inkling 2 Small and the looped Inkling 2 Mini.
const NATIVE_MODEL_TYPES: &[&str] = &["inkling2", "inkling2_loop"];

/// What an Inkling checkpoint's config says about loading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InklingCheckpoint {
    model_type: String,
    fixed_loop: bool,
    native_quantization: bool,
}

impl InklingCheckpoint {
    /// The Inkling checkpoint `record`'s snapshot holds, or `None` for any
    /// other model, or when its config cannot be read.
    pub(super) fn of(record: &ModelRecord) -> Option<Self> {
        let snapshot = SidecarModelPaths::resolve(record).snapshot;
        Self::in_snapshot(Path::new(&snapshot))
    }

    fn in_snapshot(snapshot: &Path) -> Option<Self> {
        let bytes = kernel::fs::read_regular(&snapshot.join("config.json")).ok()?;
        let config: Value = serde_json::from_slice(&bytes).ok()?;
        let model_type = config.get("model_type")?.as_str()?;
        if !model_type.starts_with("inkling") {
            return None;
        }
        let loop_config = config.pointer("/text_config/loop_config");
        let variable = loop_config
            .and_then(|loops| loops.get("is_varloop"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // Upstream refuses a loop it cannot run as a fixed number of passes,
        // and a looped model type that names no loop at all.
        let fixed_loop = !variable && (model_type != "inkling2_loop" || loop_config.is_some());
        // A ModelOpt checkpoint names its per-expert formats (NVFP4, MXFP8)
        // only in its quantization sidecar; without it the experts misload.
        let modelopt = config
            .pointer("/quantization_config/quant_method")
            .and_then(Value::as_str)
            == Some("modelopt");
        let native_quantization = !modelopt || snapshot.join("hf_quant_config.json").is_file();
        Some(Self {
            model_type: model_type.to_owned(),
            fixed_loop,
            native_quantization,
        })
    }

    /// Whether mlx-vlm loads this checkpoint once converted to MLX.
    pub(super) fn loads_converted(&self) -> bool {
        self.fixed_loop
    }

    /// Whether mlx-vlm loads this checkpoint's original snapshot in place.
    pub(super) fn loads_native(&self) -> bool {
        NATIVE_MODEL_TYPES.contains(&self.model_type.as_str())
            && self.fixed_loop
            && self.native_quantization
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A directory under the system temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "hedos-inkling-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn checkpoint(config: Value, quant_sidecar: bool) -> Option<InklingCheckpoint> {
        let dir = Scratch::new();
        std::fs::write(dir.0.join("config.json"), config.to_string()).unwrap();
        if quant_sidecar {
            std::fs::write(dir.0.join("hf_quant_config.json"), "{}").unwrap();
        }
        InklingCheckpoint::in_snapshot(&dir.0)
    }

    fn mini(is_varloop: bool) -> Value {
        json!({
            "model_type": "inkling2_loop",
            "quantization_config": {"quant_method": "modelopt"},
            "text_config": {"loop_config": {"is_varloop": is_varloop}},
        })
    }

    #[test]
    fn the_native_mini_snapshot_loads_with_its_quantization_sidecar() {
        let found = checkpoint(mini(false), true).unwrap();
        assert!(found.loads_native());
        assert!(found.loads_converted());
        assert!(!checkpoint(mini(false), false).unwrap().loads_native());
    }

    #[test]
    fn a_variable_loop_loads_in_no_form() {
        let found = checkpoint(mini(true), true).unwrap();
        assert!(!found.loads_native());
        assert!(!found.loads_converted());
    }

    #[test]
    fn a_looped_model_type_without_a_loop_does_not_load() {
        let found = checkpoint(json!({"model_type": "inkling2_loop"}), false).unwrap();
        assert!(!found.loads_converted());
    }

    #[test]
    fn small_loads_natively_and_the_first_generation_only_converted() {
        let small = checkpoint(json!({"model_type": "inkling2"}), false).unwrap();
        assert!(small.loads_native());
        let first = checkpoint(json!({"model_type": "inkling_mm_model"}), false).unwrap();
        assert!(first.loads_converted());
        assert!(!first.loads_native());
    }

    #[test]
    fn another_model_or_an_unreadable_config_is_not_inkling() {
        assert!(checkpoint(json!({"model_type": "qwen3_vl"}), false).is_none());
        assert!(checkpoint(json!({}), false).is_none());
        assert!(InklingCheckpoint::in_snapshot(&Scratch::new().0).is_none());
    }
}
