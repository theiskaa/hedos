//! Apple's on-device model (Apple Intelligence) as a runtime: a backend seam
//! over the Swift `FoundationModels` bridge and the adapter that serves chat
//! and completion through it.

mod adapter;
mod backend;
mod scanner;

pub use adapter::AppleFoundationAdapter;
pub use backend::{
    AppleFoundationBackend, BuiltinAvailability, BuiltinEvent, BuiltinEventStream, BuiltinOptions,
    MissingAppleBackend,
};
pub use scanner::AppleFoundationScanner;
