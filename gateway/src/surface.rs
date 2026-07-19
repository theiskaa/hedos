//! The two wire dialects the gateway speaks. Every request is served on one of
//! them, and errors and payloads are shaped to match.

/// The API dialect a request is served on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewaySurface {
    /// OpenAI-compatible (`/v1/...`): errors nest under an `error` object.
    OpenAI,
    /// Ollama-compatible (`/api/...`): errors are a flat `{"error": message}`.
    Ollama,
}
