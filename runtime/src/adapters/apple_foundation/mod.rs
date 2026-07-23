//! Apple's on-device model (Apple Intelligence) as a runtime: a backend seam
//! over the Swift `FoundationModels` bridge, the adapter that serves chat and
//! completion through it, and the discovery scanner that puts the model on the
//! shelf when it is available.

mod adapter;
mod backend;
#[cfg(target_os = "macos")]
mod ffi;
mod scanner;
// Compiled everywhere so its tests pin the FFI wire protocol on every
// platform, though only the macOS-only ffi module consumes it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod wire;

pub use adapter::AppleFoundationAdapter;
pub use backend::{AppleFoundationBackend, MissingAppleBackend};
#[cfg(target_os = "macos")]
pub use ffi::loaded_apple_backend;
pub use scanner::AppleFoundationScanner;
