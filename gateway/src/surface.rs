//! The wire dialects the gateway speaks. Every request is served on one of
//! them, and errors and payloads are shaped to match.

/// The API dialect a request is served on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GatewaySurface {
    /// OpenAI-compatible (`/v1/...`): errors nest under an `error` object.
    OpenAI,
    /// Ollama-compatible (`/api/...`): errors are a flat `{"error": message}`.
    Ollama,
    /// Anthropic-compatible (`/v1/messages`): errors are `{"type": "error",
    /// "error": {...}}`.
    Anthropic,
}

impl GatewaySurface {
    /// The surface a request path is served on: anything under `/api` is Ollama,
    /// `/v1/messages` is Anthropic, and everything else is OpenAI.
    pub fn for_path(path: &str) -> Self {
        if path.starts_with("/api") {
            GatewaySurface::Ollama
        } else if path.starts_with("/v1/messages") {
            GatewaySurface::Anthropic
        } else {
            GatewaySurface::OpenAI
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_paths_are_ollama_others_are_openai() {
        assert_eq!(
            GatewaySurface::for_path("/api/chat"),
            GatewaySurface::Ollama
        );
        assert_eq!(
            GatewaySurface::for_path("/api/tags"),
            GatewaySurface::Ollama
        );
        assert_eq!(
            GatewaySurface::for_path("/v1/chat/completions"),
            GatewaySurface::OpenAI
        );
        assert_eq!(GatewaySurface::for_path("/"), GatewaySurface::OpenAI);
    }

    #[test]
    fn only_the_messages_path_is_anthropic() {
        assert_eq!(
            GatewaySurface::for_path("/v1/messages"),
            GatewaySurface::Anthropic
        );
        // Inference posts to /v1/messages?beta=true, and the router matches on
        // the path, so the query string never reaches this.
        assert_eq!(
            GatewaySurface::for_path("/v1/messages/count_tokens"),
            GatewaySurface::Anthropic
        );
        // The models list stays OpenAI-shaped; both dialects read `data[].id`.
        assert_eq!(
            GatewaySurface::for_path("/v1/models"),
            GatewaySurface::OpenAI
        );
    }
}
