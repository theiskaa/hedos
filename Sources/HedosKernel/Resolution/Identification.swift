import Foundation

public enum ModelFormat: String, Sendable, Hashable {
    case gguf
    case safetensors
    case mlxSafetensors
    case diffusers
    case ollamaStore
    case unknown
}

public struct IdentifiedModel: Sendable, Hashable {
    public var format: ModelFormat
    public var modality: Modality?
    public var capabilities: [Capability]
    public var execution: ExecutionMode
    public var params: [ParamSpec] = []
    public var pipelineClass: String? = nil
}

public struct DiffusersPipelineProfile: Sendable, Hashable {
    public var modality: Modality
    public var capabilities: [Capability]
    public var params: [ParamSpec]
}

public enum Identification {
    public static func identify(_ record: ModelRecord) -> IdentifiedModel {
        if record.source.kind == .ollama {
            return IdentifiedModel(
                format: .ollamaStore,
                modality: .text,
                capabilities: [.chat, .complete],
                execution: .stream)
        }

        let base = URL(fileURLWithPath: (record.source.path as NSString).expandingTildeInPath)
        let container = containerURL(for: base, record: record)

        if base.pathExtension.lowercased() == "gguf" || hasGGUFMagic(at: base) {
            return IdentifiedModel(
                format: .gguf,
                modality: .text,
                capabilities: [.chat, .complete],
                execution: .stream)
        }

        let modelIndexURL = container.appendingPathComponent("model_index.json")
        if FileManager.default.fileExists(atPath: modelIndexURL.path) {
            let pipelineClass = diffusersPipelineClass(at: modelIndexURL)
            guard let pipelineClass,
                let profile = diffusersPipelineProfiles[pipelineClass]
            else {
                return IdentifiedModel(
                    format: .diffusers,
                    modality: nil,
                    capabilities: [],
                    execution: .job,
                    pipelineClass: pipelineClass)
            }
            return IdentifiedModel(
                format: .diffusers,
                modality: profile.modality,
                capabilities: profile.capabilities,
                execution: .job,
                params: profile.params,
                pipelineClass: pipelineClass)
        }

        let configURL = container.appendingPathComponent("config.json")
        let hint = ModalityHints.fromConfigJSON(at: configURL)
        let safetensorsFormat = safetensorsFormat(in: container, configURL: configURL)

        if let safetensorsFormat {
            return IdentifiedModel(
                format: safetensorsFormat,
                modality: hint?.modality,
                capabilities: hint?.capabilities ?? [],
                execution: hint?.execution ?? .sync)
        }
        if let hint {
            return IdentifiedModel(
                format: .unknown,
                modality: hint.modality,
                capabilities: hint.capabilities,
                execution: hint.execution)
        }
        return IdentifiedModel(
            format: .unknown, modality: nil, capabilities: [], execution: .sync)
    }

    static let imagePipelineParams: [ParamSpec] = [
        ParamSpec(key: "steps", type: .int, defaultValue: .int(4), range: [.int(1), .int(50)]),
        ParamSpec(
            key: "guidance", type: .float, defaultValue: .double(4.0),
            range: [.double(0), .double(10)]),
        ParamSpec(
            key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
            values: ["512x512", "768x768", "1024x1024"]),
        ParamSpec(key: "seed", type: .int),
    ]

    static let diffusersPipelineProfiles: [String: DiffusersPipelineProfile] = [
        "FluxPipeline": DiffusersPipelineProfile(
            modality: .image,
            capabilities: [.image],
            params: imagePipelineParams)
    ]

    static func diffusersPipelineClass(at url: URL) -> String? {
        guard let data = try? Data(contentsOf: url),
            let index = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return index["_class_name"] as? String
    }

    static func hasGGUFMagic(at url: URL) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let magic = try? handle.read(upToCount: 4)
        else { return false }
        try? handle.close()
        return magic == Data("GGUF".utf8)
    }

    static func safetensorsHeaderMetadataFormat(at url: URL) -> String? {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let lengthData = try? handle.read(upToCount: 8), lengthData.count == 8
        else { return nil }
        defer { try? handle.close() }
        let length = lengthData.withUnsafeBytes { $0.load(as: UInt64.self) }
        guard length > 0, length < 100_000_000,
            let headerData = try? handle.read(upToCount: Int(length)),
            let header = try? JSONSerialization.jsonObject(with: headerData) as? [String: Any],
            let metadata = header["__metadata__"] as? [String: Any]
        else { return nil }
        return metadata["format"] as? String
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

    private static func safetensorsFormat(in container: URL, configURL: URL) -> ModelFormat? {
        let files =
            (try? FileManager.default.contentsOfDirectory(
                at: container, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]))
            ?? []
        guard let weight = files.first(where: { $0.pathExtension == "safetensors" }) else {
            return nil
        }

        if let config = try? JSONSerialization.jsonObject(
            with: Data(contentsOf: configURL)) as? [String: Any],
            config["quantization"] != nil
        {
            return .mlxSafetensors
        }
        if safetensorsHeaderMetadataFormat(at: weight.resolvingSymlinksInPath()) == "mlx" {
            return .mlxSafetensors
        }
        return .safetensors
    }
}
