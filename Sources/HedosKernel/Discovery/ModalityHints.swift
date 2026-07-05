import Foundation

/// Cheap, file-shape-based modality hints used at discovery time. This is
/// NOT the resolution engine (M4) — just enough archaeology that the shelf
/// can group a freshly found model sensibly.
enum ModalityHints {
    struct Hint {
        var modality: Modality?
        var capabilities: [Capability]
        var execution: ExecutionMode
    }

    /// A `model_index.json` marks a diffusers-style pipeline: image-shaped.
    static func fromModelIndex(at url: URL) -> Hint {
        Hint(modality: .image, capabilities: [.image], execution: .job)
    }

    /// `config.json` architectures archaeology, cheapest-first.
    static func fromConfigJSON(at url: URL) -> Hint? {
        guard let data = try? Data(contentsOf: url),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        let architectures = (json["architectures"] as? [String]) ?? []

        for arch in architectures {
            if arch.contains("Kokoro") || arch.contains("StyleTTS") {
                return Hint(modality: .speech, capabilities: [.speak], execution: .stream)
            }
            if arch.contains("Whisper") {
                return Hint(modality: .audio, capabilities: [.transcribe], execution: .stream)
            }
            if arch.hasSuffix("ForCausalLM") || arch.contains("LMHead") {
                return Hint(modality: .text, capabilities: [.chat, .complete], execution: .stream)
            }
        }

        // Shape heuristics for configs with no architectures[] at all.
        // Real Kokoro/StyleTTS2 configs are exactly this: istftnet/plbert/
        // style_dim/n_mels keys and nothing transformers-shaped.
        let keys = Set(json.keys)
        if keys.contains("istftnet") || keys.contains("plbert")
            || (keys.contains("style_dim") && keys.contains("n_mels"))
        {
            return Hint(modality: .speech, capabilities: [.speak], execution: .stream)
        }
        return nil
    }

    /// A GGUF file is a text model until the resolution engine says otherwise.
    static var gguf: Hint {
        Hint(modality: .text, capabilities: [.chat, .complete], execution: .stream)
    }
}
