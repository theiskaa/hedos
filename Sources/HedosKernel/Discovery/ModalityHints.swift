import Foundation

enum ModalityHints {
    struct Hint {
        var modality: Modality?
        var capabilities: [Capability]
        var execution: ExecutionMode
    }

    static func fromModelIndex(at url: URL) -> Hint {
        Hint(modality: .image, capabilities: [.image], execution: .job)
    }

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

        let keys = Set(json.keys)
        if keys.contains("istftnet") || keys.contains("plbert")
            || (keys.contains("style_dim") && keys.contains("n_mels"))
        {
            return Hint(modality: .speech, capabilities: [.speak], execution: .stream)
        }
        return nil
    }

    static var gguf: Hint {
        Hint(modality: .text, capabilities: [.chat, .complete], execution: .stream)
    }

    static var whisperBin: Hint {
        Hint(modality: .audio, capabilities: [.transcribe], execution: .stream)
    }
}
