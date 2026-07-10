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
        ParamSpec(key: "max_tokens", type: .int, range: [.int(1), .int(4096)]),
    ]

    static let endpointParams: [ParamSpec] = [
        ParamSpec(key: "temperature", type: .float, range: [.double(0), .double(2)]),
        ParamSpec(key: "top_p", type: .float, range: [.double(0), .double(1)]),
        ParamSpec(key: "max_tokens", type: .int, range: [.int(1), .int(32768)]),
    ]

    static func diffusersPipelineClass(at url: URL) -> String? {
        guard let data = try? Data(contentsOf: url),
            let index = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return index["_class_name"] as? String
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

    static func hasGGUFMagic(at url: URL) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let magic = try? handle.read(upToCount: 4)
        else { return false }
        try? handle.close()
        return magic == Data("GGUF".utf8)
    }

    static func hasGGMLMagic(at url: URL) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let magic = try? handle.read(upToCount: 4)
        else { return false }
        try? handle.close()
        return magic == Data("lmgg".utf8)
    }

    static let ggufArchitectureProfiles: [String: GGUFArchitectureProfile] = [
        "whisper": GGUFArchitectureProfile(
            modality: .audio,
            capabilities: [.transcribe],
            execution: .stream),
        "qwen2vl": GGUFArchitectureProfile(
            modality: .text,
            capabilities: [.chat, .complete, .see],
            execution: .stream),
        "mllama": GGUFArchitectureProfile(
            modality: .text,
            capabilities: [.chat, .complete, .see],
            execution: .stream),
        "clip": GGUFArchitectureProfile(
            modality: .vision,
            capabilities: [],
            execution: .sync),
        "bert": GGUFArchitectureProfile(
            modality: .embedding,
            capabilities: [.embed],
            execution: .stream),
        "nomic-bert": GGUFArchitectureProfile(
            modality: .embedding,
            capabilities: [.embed],
            execution: .stream),
    ]

    static let ollamaChatProfile = GGUFArchitectureProfile(
        modality: .text, capabilities: [.chat, .complete], execution: .stream)

    static let ollamaVisionProfile = GGUFArchitectureProfile(
        modality: .text, capabilities: [.chat, .complete, .see], execution: .stream)

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

    static func ggufGeneralArchitecture(at url: URL) -> String? {
        ggufFacts(at: url)?.architecture
    }

    static func ggufFacts(at url: URL) -> GGUFFacts? {
        guard let reader = GGUFHeaderReader(path: url.path) else { return nil }
        guard let magic = reader.readBytes(4), magic.elementsEqual(Array("GGUF".utf8)),
            let version: UInt32 = readLittleEndian(reader), version >= 2,
            let _: UInt64 = readLittleEndian(reader),
            let keyValueCount: UInt64 = readLittleEndian(reader)
        else { return nil }

        var architecture: String?
        var contextLengths: [String: Int] = [:]
        var hasChatTemplate = false

        walk: for _ in 0..<min(keyValueCount, 512) {
            guard let key = readGGUFString(reader),
                let valueType: UInt32 = readLittleEndian(reader)
            else { break walk }
            if key == "general.architecture" {
                guard valueType == 8, let value = readGGUFString(reader) else { break walk }
                architecture = value
            } else if key == "tokenizer.chat_template" {
                hasChatTemplate = true
                guard skipGGUFValue(reader, type: valueType) else { break walk }
            } else if key.hasSuffix(".context_length"),
                let value = readGGUFInteger(reader, type: valueType)
            {
                contextLengths[key] = value
            } else {
                guard skipGGUFValue(reader, type: valueType) else { break walk }
            }
        }

        var contextLength: Int?
        if let architecture, let matched = contextLengths["\(architecture).context_length"] {
            contextLength = matched
        } else if contextLengths.count == 1 {
            contextLength = contextLengths.values.first
        }
        return GGUFFacts(
            architecture: architecture,
            contextLength: contextLength,
            hasChatTemplate: hasChatTemplate)
    }

    private final class GGUFHeaderReader {
        private let handle: FileHandle
        private var buffer: [UInt8] = []
        private var cursor = 0

        init?(path: String) {
            guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
            self.handle = handle
        }

        deinit {
            try? handle.close()
        }

        func readBytes(_ count: Int) -> ArraySlice<UInt8>? {
            guard count >= 0, fill(count) else { return nil }
            defer { cursor += count }
            return buffer[cursor..<cursor + count]
        }

        func skip(_ count: UInt64) -> Bool {
            let available = UInt64(buffer.count - cursor)
            if count <= available {
                cursor += Int(count)
                return true
            }
            let beyond = count - available
            buffer.removeAll(keepingCapacity: true)
            cursor = 0
            guard let current = try? handle.offset() else { return false }
            let (target, overflow) = current.addingReportingOverflow(beyond)
            guard !overflow else { return false }
            return (try? handle.seek(toOffset: target)) != nil
        }

        private func fill(_ count: Int) -> Bool {
            while buffer.count - cursor < count {
                guard let more = try? handle.read(upToCount: max(1 << 20, count)),
                    !more.isEmpty
                else { return false }
                if cursor > 0 {
                    buffer.removeFirst(cursor)
                    cursor = 0
                }
                buffer.append(contentsOf: more)
            }
            return true
        }
    }

    private static func readGGUFInteger(_ reader: GGUFHeaderReader, type: UInt32) -> Int? {
        switch type {
        case 4: (readLittleEndian(reader) as UInt32?).map { Int($0) }
        case 5: (readLittleEndian(reader) as Int32?).map { Int($0) }
        case 10: (readLittleEndian(reader) as UInt64?).map { Int(clamping: $0) }
        case 11: (readLittleEndian(reader) as Int64?).map { Int(clamping: $0) }
        default: nil
        }
    }

    private static func readLittleEndian<T: FixedWidthInteger>(
        _ reader: GGUFHeaderReader
    ) -> T? {
        let size = MemoryLayout<T>.size
        guard let bytes = reader.readBytes(size) else { return nil }
        var value: T = 0
        withUnsafeMutableBytes(of: &value) { $0.copyBytes(from: bytes) }
        return T(littleEndian: value)
    }

    private static func readGGUFString(_ reader: GGUFHeaderReader) -> String? {
        guard let length: UInt64 = readLittleEndian(reader), length <= 1 << 16,
            let bytes = reader.readBytes(Int(length))
        else { return nil }
        return String(decoding: bytes, as: UTF8.self)
    }

    private static func skipGGUFValue(_ reader: GGUFHeaderReader, type: UInt32) -> Bool {
        if let width = ggufScalarWidth(type: type) {
            return reader.skip(width)
        }
        switch type {
        case 8:
            guard let length: UInt64 = readLittleEndian(reader) else { return false }
            return reader.skip(length)
        case 9:
            guard let elementType: UInt32 = readLittleEndian(reader),
                let count: UInt64 = readLittleEndian(reader)
            else { return false }
            if let width = ggufScalarWidth(type: elementType) {
                let (total, overflow) = count.multipliedReportingOverflow(by: width)
                return !overflow && reader.skip(total)
            }
            guard elementType == 8, count <= 1 << 24 else { return false }
            for _ in 0..<count {
                guard let length: UInt64 = readLittleEndian(reader), reader.skip(length)
                else { return false }
            }
            return true
        default:
            return false
        }
    }

    private static func ggufScalarWidth(type: UInt32) -> UInt64? {
        switch type {
        case 0, 1, 7: 1
        case 2, 3: 2
        case 4, 5, 6: 4
        case 10, 11, 12: 8
        default: nil
        }
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
