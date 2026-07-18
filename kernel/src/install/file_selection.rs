//! Classifying repository files by path. The full Hugging Face file selection
//! (GGUF shard grouping, quantization preference, diffusers/transformers picking)
//! is deferred with the `HFSibling` + GGUF-shard types it needs; these are the
//! pure path classifiers it and the [install plan](crate::install::plan) build on.

const WEIGHT_EXTENSIONS: [&str; 6] = ["safetensors", "gguf", "bin", "ckpt", "pt", "pth"];

/// Whether `path`'s extension marks it a model weight file.
pub fn is_weight_path(path: &str) -> bool {
    WEIGHT_EXTENSIONS.contains(&file_extension(path).as_str())
}

/// The lowercased extension of `path` (the part after the last `.`), or empty when
/// there is none or the only dot is a leading one (a dotfile has no extension).
pub fn file_extension(path: &str) -> String {
    match path.rfind('.') {
        Some(dot) if dot > 0 => path[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}
