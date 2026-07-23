//! The in-process MLX-Swift runtime: a backend seam over the Swift MLX bridge,
//! and the adapter that serves chat and completion through it for
//! MLX-safetensors text models. Unlike the Apple bridge there is no discovery
//! scanner — MLX models already reach the shelf through the filesystem
//! scanners; this runtime only competes to *serve* them, winning the bid over
//! the Python `mlx-lm` sidecar when the in-process engine is available.

mod adapter;
mod backend;
#[cfg(target_os = "macos")]
mod ffi;
// Compiled everywhere so its tests pin the FFI wire protocol on every platform,
// though only the macOS-only ffi module consumes it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod wire;

pub use adapter::MlxSwiftAdapter;
pub use backend::{MissingMlxSwiftBackend, MlxSwiftBackend};
#[cfg(target_os = "macos")]
pub use ffi::loaded_mlx_swift_backend;
