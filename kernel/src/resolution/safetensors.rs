//! Reading a safetensors header to tell an MLX-format weight directory from a
//! plain one.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use crate::resolution::format::ModelFormat;

const MAX_HEADER_LEN: u64 = 100_000_000;

/// The `__metadata__.format` value from a safetensors file header, if present.
/// A safetensors file begins with an 8-byte little-endian header length followed
/// by that many bytes of JSON.
pub fn safetensors_header_format(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut length_bytes = [0u8; 8];
    file.read_exact(&mut length_bytes).ok()?;
    let length = u64::from_le_bytes(length_bytes);
    if length == 0 || length >= MAX_HEADER_LEN {
        return None;
    }
    let mut header = Vec::new();
    if file.take(length).read_to_end(&mut header).ok()? as u64 != length {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&header).ok()?;
    json.get("__metadata__")?
        .get("format")?
        .as_str()
        .map(str::to_owned)
}

/// Classify a safetensors weight directory as MLX or plain. A `quantization` key
/// in the config, or a header `format` of `mlx`, marks an MLX directory.
pub fn safetensors_format(container: &Path, config_path: &Path) -> Option<ModelFormat> {
    let weight = fs::read_dir(container)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "safetensors"))?;

    if let Ok(bytes) = fs::read(config_path)
        && let Ok(config) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && config.get("quantization").is_some()
    {
        return Some(ModelFormat::MlxSafetensors);
    }

    let weight = fs::canonicalize(&weight).unwrap_or(weight);
    if safetensors_header_format(&weight).as_deref() == Some("mlx") {
        return Some(ModelFormat::MlxSafetensors);
    }
    Some(ModelFormat::Safetensors)
}
