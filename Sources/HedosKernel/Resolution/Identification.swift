import Foundation

public enum ModelFormat: String, Sendable, Hashable {
    case gguf
    case ggmlBin
    case safetensors
    case mlxSafetensors
    case diffusers
    case ollamaStore
    case builtin
    case endpoint
    case unknown
}

public struct IdentifiedModel: Sendable, Hashable {
    public var format: ModelFormat
    public var modality: Modality?
    public var capabilities: [Capability]
    public var execution: ExecutionMode
    public var params: [ParamSpec] = []
    public var pipelineClass: String? = nil
    public var contextLength: Int? = nil
    public var hasChatTemplate: Bool? = nil
}

public struct GGUFFacts: Sendable, Hashable {
    public var architecture: String?
    public var contextLength: Int?
    public var hasChatTemplate: Bool
}

public struct GGUFArchitectureProfile: Sendable, Hashable {
    public var modality: Modality
    public var capabilities: [Capability]
    public var execution: ExecutionMode
}

public enum Identification {
    public static func identify(
        _ record: ModelRecord, pipelines: PipelineFamilyRegistry = .builtin
    ) -> IdentifiedModel {
        if record.source.kind == .builtin {
            return IdentifiedModel(
                format: .builtin,
                modality: .text,
                capabilities: [.chat, .complete],
                execution: .stream,
                params: builtinParams)
        }
        if record.source.kind == .endpoint {
            return IdentifiedModel(
                format: .endpoint,
                modality: .text,
                capabilities: [.chat, .complete],
                execution: .stream,
                params: endpointParams)
        }
        if record.source.kind == .ollama {
            let profile = ollamaProfile(
                hasProjector: manifestHasProjectorLayer(at: record.source.path),
                blobPath: record.primaryWeightPath)
            return IdentifiedModel(
                format: .ollamaStore,
                modality: profile.modality,
                capabilities: profile.capabilities,
                execution: profile.execution)
        }

        let base = URL(fileURLWithPath: (record.source.path as NSString).expandingTildeInPath)
        let container = containerURL(for: base, record: record)

        if base.pathExtension.lowercased() == "bin" && hasGGMLMagic(at: base) {
            return IdentifiedModel(
                format: .ggmlBin,
                modality: .audio,
                capabilities: [.transcribe],
                execution: .stream)
        }

        if base.pathExtension.lowercased() == "gguf" || hasGGUFMagic(at: base) {
            if isMmprojName(base.lastPathComponent) {
                return IdentifiedModel(
                    format: .gguf,
                    modality: clipProfile.modality,
                    capabilities: clipProfile.capabilities,
                    execution: clipProfile.execution)
            }
            let facts = ggufFacts(at: base)
            if let architecture = facts?.architecture,
                let profile = ggufArchitectureProfiles[architecture]
            {
                return IdentifiedModel(
                    format: .gguf,
                    modality: profile.modality,
                    capabilities: profile.capabilities,
                    execution: profile.execution,
                    contextLength: facts?.contextLength,
                    hasChatTemplate: facts?.hasChatTemplate)
            }
            let capabilities: [Capability] =
                hasMmprojCompanion(besides: base)
                ? [.chat, .complete, .see] : [.chat, .complete]
            return IdentifiedModel(
                format: .gguf,
                modality: .text,
                capabilities: capabilities,
                execution: .stream,
                contextLength: facts?.contextLength,
                hasChatTemplate: facts?.hasChatTemplate)
        }

        let modelIndexURL = container.appendingPathComponent("model_index.json")
        if FileManager.default.fileExists(atPath: modelIndexURL.path) {
            let pipelineClass = diffusersPipelineClass(at: modelIndexURL)
            let scheduler = schedulerFacts(in: container)
            let repoHint = record.source.repo ?? record.name
            guard let pipelineClass,
                let profile = pipelines.profile(
                    for: pipelineClass, scheduler: scheduler, repoHint: repoHint)
            else {
                return IdentifiedModel(
                    format: .diffusers,
                    modality: nil,
                    capabilities: [],
                    execution: .job,
                    pipelineClass: pipelineClass)
            }
            var params = profile.params
            if pipelineClass == "FluxPipeline", !fluxUsesGuidance(in: container) {
                params.removeAll { $0.key == "guidance" }
            }
            return IdentifiedModel(
                format: .diffusers,
                modality: profile.modality,
                capabilities: profile.capabilities,
                execution: .job,
                params: params,
                pipelineClass: pipelineClass)
        }

        let configURL = container.appendingPathComponent("config.json")
        let hint = ModalityHints.fromConfigJSON(at: configURL)
        let safetensorsFormat = Self.safetensorsFormat(in: container, configURL: configURL)

        if let safetensorsFormat {
            if hint?.modality == nil || hint?.modality == .text,
                hasSentenceTransformersLayout(in: container)
            {
                return IdentifiedModel(
                    format: safetensorsFormat,
                    modality: .embedding,
                    capabilities: [.embed],
                    execution: .stream,
                    contextLength: hint?.contextLength)
            }
            return IdentifiedModel(
                format: safetensorsFormat,
                modality: hint?.modality,
                capabilities: hint?.capabilities ?? [],
                execution: hint?.execution ?? .sync,
                contextLength: hint?.contextLength)
        }
        if let hint {
            return IdentifiedModel(
                format: .unknown,
                modality: hint.modality,
                capabilities: hint.capabilities,
                execution: hint.execution,
                contextLength: hint.contextLength)
        }
        return IdentifiedModel(
            format: .unknown, modality: nil, capabilities: [], execution: .sync)
    }

    static let builtinParams: [ParamSpec] = [
        ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)]),
        ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)]),
        ParamSpec(key: "top_k", type: .int, range: [.int(0), .int(100)]),
        ParamSpec(key: "max_tokens", type: .int, range: [.int(1), .int(4096)]),
        ParamSpec(key: "seed", type: .int),
    ]

    static let endpointParams: [ParamSpec] = [
        ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)]),
        ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)]),
        ParamSpec(key: "max_tokens", type: .int, range: [.int(1), .int(32768)]),
        ParamSpec(key: "stop", type: .string),
        ParamSpec(key: "seed", type: .int),
        ParamSpec(key: "frequency_penalty", type: .float, range: [.double(-2), .double(2)]),
        ParamSpec(key: "presence_penalty", type: .float, range: [.double(-2), .double(2)]),
    ]

    static func diffusersPipelineClass(at url: URL) -> String? {
        guard let data = try? Data(contentsOf: url),
            let index = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return index["_class_name"] as? String
    }

    static func fluxUsesGuidance(in container: URL) -> Bool {
        let url = container.appendingPathComponent("transformer/config.json")
        guard let data = try? Data(contentsOf: url),
            let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let flag = config["guidance_embeds"] as? Bool
        else { return false }
        return flag
    }

    static func schedulerFacts(in container: URL) -> SchedulerFacts? {
        let url = container.appendingPathComponent("scheduler/scheduler_config.json")
        guard let data = try? Data(contentsOf: url),
            let config = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return SchedulerFacts(
            className: config["_class_name"] as? String,
            timestepSpacing: config["timestep_spacing"] as? String)
    }

    static func ollamaProfile(hasProjector: Bool, blobPath: String?) -> GGUFArchitectureProfile {
        if hasProjector { return ollamaVisionProfile }
        if let blobPath,
            let architecture = ggufGeneralArchitecture(
                at: URL(fileURLWithPath: (blobPath as NSString).expandingTildeInPath)),
            let profile = ggufArchitectureProfiles[architecture]
        {
            return profile
        }
        return ollamaChatProfile
    }

    static func manifestHasProjectorLayer(at path: String) -> Bool {
        guard
            let data = FileManager.default.contents(
                atPath: (path as NSString).expandingTildeInPath),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let layers = object["layers"] as? [[String: Any]]
        else { return false }
        return layers.contains { ($0["mediaType"] as? String)?.hasSuffix(".projector") == true }
    }

    static let clipProfile = GGUFArchitectureProfile(
        modality: .vision, capabilities: [], execution: .sync)

    static func isMmprojName(_ name: String) -> Bool {
        name.lowercased().contains("mmproj")
    }

    static let sentenceTransformersMarkers: Set<String> = [
        "config_sentence_transformers.json", "1_Pooling",
    ]

    static func hasSentenceTransformersLayout(in container: URL) -> Bool {
        let names =
            (try? FileManager.default.contentsOfDirectory(atPath: container.path)) ?? []
        return names.contains { sentenceTransformersMarkers.contains($0) }
    }

    static func hasMmprojCompanion(besides base: URL) -> Bool {
        let directory = base.deletingLastPathComponent()
        let entries =
            (try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]))
            ?? []
        return entries.contains { entry in
            entry.pathExtension.lowercased() == "gguf"
                && isMmprojName(entry.lastPathComponent)
                && entry.lastPathComponent != base.lastPathComponent
        }
    }

    private static func containerURL(for base: URL, record: ModelRecord) -> URL {
        if record.source.kind == .huggingfaceCache {
            let snapshots = base.appendingPathComponent("snapshots")
            if let ref = record.source.ref {
                let snapshot = snapshots.appendingPathComponent(ref)
                if FileManager.default.fileExists(atPath: snapshot.path) { return snapshot }
            }
        }
        return base
    }
}
