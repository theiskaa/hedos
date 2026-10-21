//! Shared helpers for the `runtime` integration tests. Centralizes the temp-dir
//! fixture every test file needs. Per the Rust Book, `tests/support/mod.rs` is
//! the canonical place for integration-test helper code — it is not compiled as
//! its own test binary, only pulled in via `mod support;`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory that removes itself when dropped. Built without
/// the `tempfile` crate to honor the project's minimal-dependency policy.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a fresh, process-unique temporary directory.
    pub fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("hedos-test-{pid}-{nanos}-{unique}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path to `name` inside this directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A GGUF string: its little-endian length, then its bytes.
fn gguf_string(value: &str) -> Vec<u8> {
    let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
    bytes.extend_from_slice(value.as_bytes());
    bytes
}

/// A GGUF header key holding a string.
pub fn kv_string(key: &str, value: &str) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend(gguf_string(value));
    bytes
}

/// A GGUF header key holding a `u32`.
pub fn kv_u32(key: &str, value: u32) -> Vec<u8> {
    let mut bytes = gguf_string(key);
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

/// A version 3 GGUF header with no tensors and the given key-value pairs.
pub fn gguf(kvs: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    for kv in kvs {
        bytes.extend_from_slice(kv);
    }
    bytes
}

/// A vision projector llama.cpp could load: its header marks an image encoder
/// and lists one tensor `width` wide, whose data follows, `padding` bytes and
/// one more.
pub fn projector_gguf(width: u64, padding: usize) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    bytes.extend(kv_string("general.architecture", "clip"));
    bytes.extend(gguf_string("clip.has_vision_encoder"));
    bytes.extend_from_slice(&7u32.to_le_bytes());
    bytes.push(1);
    bytes.extend(gguf_string("mm.2.weight"));
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.resize(bytes.len().div_ceil(32) * 32 + padding + 1, 0);
    bytes
}

/// An Ollama store under `root` holding one model, laid out as the daemon
/// writes it: a manifest naming the weight blob and, when given, a projector
/// blob. Returns the manifest path and the weight blob path, as discovery
/// records them.
pub fn ollama_store_model(
    root: &Path,
    weights: &[u8],
    projector: Option<&[u8]>,
) -> (PathBuf, PathBuf) {
    let blobs = root.join("blobs");
    std::fs::create_dir_all(&blobs).unwrap();
    let model_digest = format!("sha256:{}", "a".repeat(64));
    let weight_blob = blobs.join(model_digest.replace(':', "-"));
    std::fs::write(&weight_blob, weights).unwrap();
    let mut layers = vec![format!(
        r#"{{"mediaType":"application/vnd.ollama.image.model","digest":"{model_digest}","size":{}}}"#,
        weights.len()
    )];
    if let Some(projector) = projector {
        let digest = format!("sha256:{}", "b".repeat(64));
        std::fs::write(blobs.join(digest.replace(':', "-")), projector).unwrap();
        layers.push(format!(
            r#"{{"mediaType":"application/vnd.ollama.image.projector","digest":"{digest}","size":{}}}"#,
            projector.len()
        ));
    }
    let manifest = root.join("manifests/registry.ollama.ai/library/model/latest");
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    std::fs::write(&manifest, format!(r#"{{"layers":[{}]}}"#, layers.join(","))).unwrap();
    (manifest, weight_blob)
}
