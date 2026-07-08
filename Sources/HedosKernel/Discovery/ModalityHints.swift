import Foundation

enum ModalityHints {
    struct Hint {
        var modality: Modality?
        var capabilities: [Capability]
        var execution: ExecutionMode
    }

    struct ArchitectureRule {
        var contains: [String]
        var suffixes: [String]
        var hint: Hint

        func matches(_ architecture: String) -> Bool {
            contains.contains(where: architecture.contains)
                || suffixes.contains(where: architecture.hasSuffix)
        }
    }

    struct ConfigKeyRule {
        var requiredKeys: Set<String>
        var hint: Hint
    }

    static let speechHint = Hint(modality: .speech, capabilities: [.speak], execution: .stream)
    static let audioHint = Hint(modality: .audio, capabilities: [.transcribe], execution: .stream)
    static let textHint = Hint(modality: .text, capabilities: [.chat, .complete], execution: .stream)

    static let architectureRules: [ArchitectureRule] = [
        ArchitectureRule(contains: ["Kokoro", "StyleTTS"], suffixes: [], hint: speechHint),
        ArchitectureRule(contains: ["Whisper"], suffixes: [], hint: audioHint),
        ArchitectureRule(contains: ["LMHead"], suffixes: ["ForCausalLM"], hint: textHint),
    ]

    static let configKeyRules: [ConfigKeyRule] = [
        ConfigKeyRule(requiredKeys: ["istftnet"], hint: speechHint),
        ConfigKeyRule(requiredKeys: ["plbert"], hint: speechHint),
        ConfigKeyRule(requiredKeys: ["style_dim", "n_mels"], hint: speechHint),
    ]

    static func fromModelIndex(
        at url: URL, pipelines: PipelineFamilyRegistry = .builtin
    ) -> Hint {
        if let className = Identification.diffusersPipelineClass(at: url),
            let family = pipelines.family(for: className)
        {
            return Hint(
                modality: family.modality, capabilities: family.capabilities, execution: .job)
        }
        return Hint(modality: nil, capabilities: [], execution: .job)
    }

    static func fromConfigJSON(at url: URL) -> Hint? {
        guard let data = try? Data(contentsOf: url),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        let architectures = (json["architectures"] as? [String]) ?? []

        for arch in architectures {
            if let rule = architectureRules.first(where: { $0.matches(arch) }) {
                return rule.hint
            }
        }

        let keys = Set(json.keys)
        if let rule = configKeyRules.first(where: { $0.requiredKeys.isSubset(of: keys) }) {
            return rule.hint
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
