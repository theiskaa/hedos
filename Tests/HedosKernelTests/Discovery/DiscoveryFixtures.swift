import Foundation

@testable import HedosKernel

enum DiscoveryFixtures {
    static func data(bytes: Int, fill: UInt8 = 0xAB) -> Data {
        Data(repeating: fill, count: bytes)
    }


    struct OllamaTag {
        var namespace = "library"
        var model: String
        var tag: String
        var modelBytes: Int
        var extraBytes: Int = 0
        var malformed = false
        var paramsJSON: String? = nil
        var paramsBlobMissing = false
        var hasTemplateLayer = false
        var ggufArchitecture: String? = nil
        var hasProjectorLayer = false
    }

    static func makeOllamaStore(at root: URL, tags: [OllamaTag]) throws {
        let fm = FileManager.default
        let blobs = root.appendingPathComponent("blobs")
        try fm.createDirectory(at: blobs, withIntermediateDirectories: true)

        for (index, tag) in tags.enumerated() {
            let manifestDir = root.appendingPathComponent(
                "manifests/registry.ollama.ai/\(tag.namespace)/\(tag.model)")
            try fm.createDirectory(at: manifestDir, withIntermediateDirectories: true)
            let manifestURL = manifestDir.appendingPathComponent(tag.tag)

            if tag.malformed {
                try Data("{not json".utf8).write(to: manifestURL)
                continue
            }

            let digest = String(format: "%064d", index)
            let modelBlob =
                tag.ggufArchitecture.map { ggufData(architecture: $0) }
                ?? data(bytes: tag.modelBytes, fill: UInt8(0x10 + index))
            try modelBlob.write(to: blobs.appendingPathComponent("sha256-\(digest)"))

            var layers: [[String: Any]] = [
                [
                    "mediaType": "application/vnd.ollama.image.model",
                    "size": tag.modelBytes,
                    "digest": "sha256:\(digest)",
                ]
            ]
            if tag.hasProjectorLayer {
                let projectorDigest = String(format: "%064d", 700 + index)
                let blob = data(bytes: 128, fill: 0x2A)
                layers.append([
                    "mediaType": "application/vnd.ollama.image.projector",
                    "size": blob.count,
                    "digest": "sha256:\(projectorDigest)",
                ])
                try blob.write(to: blobs.appendingPathComponent("sha256-\(projectorDigest)"))
            }
            if tag.extraBytes > 0 || tag.paramsJSON != nil {
                let paramsDigest = String(format: "%064d", 900 + index)
                let blob = Data((tag.paramsJSON ?? "{}").utf8)
                layers.append([
                    "mediaType": "application/vnd.ollama.image.params",
                    "size": tag.extraBytes > 0 ? tag.extraBytes : blob.count,
                    "digest": "sha256:\(paramsDigest)",
                ])
                if !tag.paramsBlobMissing {
                    try blob.write(to: blobs.appendingPathComponent("sha256-\(paramsDigest)"))
                }
            }
            if tag.hasTemplateLayer {
                let templateDigest = String(format: "%064d", 800 + index)
                let blob = Data("{{ .Prompt }}".utf8)
                layers.append([
                    "mediaType": "application/vnd.ollama.image.template",
                    "size": blob.count,
                    "digest": "sha256:\(templateDigest)",
                ])
                try blob.write(to: blobs.appendingPathComponent("sha256-\(templateDigest)"))
            }
            let manifest: [String: Any] = [
                "schemaVersion": 2,
                "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                "config": ["mediaType": "application/vnd.docker.container.image.v1+json"],
                "layers": layers,
            ]
            try JSONSerialization.data(withJSONObject: manifest).write(to: manifestURL)
        }
    }


    struct HFRepo {
        var org: String
        var repo: String
        var files: [(name: String, bytes: Int)]
        var configJSON: String?
        var modelIndexJSON: String?
        var schedulerConfigJSON: String?
        var transformerConfigJSON: String?
        var writeRefsMain = true
        var revision = "abc123def456"
        var incompleteBlobs: [String] = []
        var safetensorsIndexJSON: String? = nil
    }

    static func makeHFRepo(at hubRoot: URL, _ spec: HFRepo) throws {
        let fm = FileManager.default
        let repoDir = hubRoot.appendingPathComponent("models--\(spec.org)--\(spec.repo)")
        let blobs = repoDir.appendingPathComponent("blobs")
        let snapshot = repoDir.appendingPathComponent("snapshots/\(spec.revision)")
        try fm.createDirectory(at: blobs, withIntermediateDirectories: true)
        try fm.createDirectory(at: snapshot, withIntermediateDirectories: true)

        if spec.writeRefsMain {
            let refs = repoDir.appendingPathComponent("refs")
            try fm.createDirectory(at: refs, withIntermediateDirectories: true)
            try spec.revision.write(
                to: refs.appendingPathComponent("main"), atomically: true, encoding: .utf8)
        }

        for (index, file) in spec.files.enumerated() {
            let blob = blobs.appendingPathComponent("blob\(index)")
            try data(bytes: file.bytes, fill: UInt8(0x40 + index)).write(to: blob)
            try fm.createSymbolicLink(
                at: snapshot.appendingPathComponent(file.name), withDestinationURL: blob)
        }
        if let config = spec.configJSON {
            let blob = blobs.appendingPathComponent("blob-config")
            try Data(config.utf8).write(to: blob)
            try fm.createSymbolicLink(
                at: snapshot.appendingPathComponent("config.json"), withDestinationURL: blob)
        }
        if let modelIndex = spec.modelIndexJSON {
            let blob = blobs.appendingPathComponent("blob-index")
            try Data(modelIndex.utf8).write(to: blob)
            try fm.createSymbolicLink(
                at: snapshot.appendingPathComponent("model_index.json"), withDestinationURL: blob)
        }
        if let scheduler = spec.schedulerConfigJSON {
            let blob = blobs.appendingPathComponent("blob-scheduler")
            try Data(scheduler.utf8).write(to: blob)
            let schedulerDir = snapshot.appendingPathComponent("scheduler")
            try fm.createDirectory(at: schedulerDir, withIntermediateDirectories: true)
            try fm.createSymbolicLink(
                at: schedulerDir.appendingPathComponent("scheduler_config.json"),
                withDestinationURL: blob)
        }
        if let transformerConfig = spec.transformerConfigJSON {
            let blob = blobs.appendingPathComponent("blob-transformer")
            try Data(transformerConfig.utf8).write(to: blob)
            let transformerDir = snapshot.appendingPathComponent("transformer")
            try fm.createDirectory(at: transformerDir, withIntermediateDirectories: true)
            try fm.createSymbolicLink(
                at: transformerDir.appendingPathComponent("config.json"),
                withDestinationURL: blob)
        }
        for (index, name) in spec.incompleteBlobs.enumerated() {
            try data(bytes: 64, fill: UInt8(0x60 + index))
                .write(to: blobs.appendingPathComponent("\(name).incomplete"))
        }
        if let indexJSON = spec.safetensorsIndexJSON {
            let blob = blobs.appendingPathComponent("blob-safetensors-index")
            try Data(indexJSON.utf8).write(to: blob)
            try fm.createSymbolicLink(
                at: snapshot.appendingPathComponent("model.safetensors.index.json"),
                withDestinationURL: blob)
        }
    }

    @discardableResult
    static func makeShardedGGUF(
        at dir: URL, baseName: String, parts: Int, presentParts: Set<Int>? = nil,
        bytesPerPart: Int = 1024
    ) throws -> [URL] {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        var written: [URL] = []
        for part in 1...parts {
            guard presentParts?.contains(part) ?? true else { continue }
            let name = String(
                format: "%@-%05d-of-%05d.gguf", baseName, part, parts)
            let url = dir.appendingPathComponent(name)
            var builder = GGUFFixtureBuilder(keyValueCount: 1)
            builder.addString(key: "general.architecture", value: "llama")
            try builder.write(to: url, trailingBytes: bytesPerPart)
            written.append(url)
        }
        return written
    }

    static let kokoroConfig =
        #"{"istftnet": {}, "plbert": {}, "style_dim": 128, "n_mels": 80, "n_token": 178}"#
    static let causalLMConfig =
        #"{"architectures": ["Qwen3ForCausalLM"], "model_type": "qwen3"}"#
    static let qwen2VLConfig =
        #"{"architectures": ["Qwen2VLForConditionalGeneration"], "vision_config": {"depth": 32}}"#
    static let nomicEmbedConfig =
        #"{"architectures": ["NomicBertModel"], "model_type": "nomic_bert"}"#
    static let barkConfig =
        #"{"architectures": ["BarkModel"], "model_type": "bark"}"#
    static let parlerTTSConfig =
        #"{"architectures": ["ParlerTTSForConditionalGeneration"], "model_type": "parler_tts"}"#
    static let mlxWhisperConfig =
        #"{"architectures": ["WhisperForConditionalGeneration"], "quantization": {"bits": 4}}"#
    static let fluxModelIndex = #"{"_class_name": "FluxPipeline"}"#
    static let fluxDevTransformerConfig = #"{"guidance_embeds": true}"#
    static let fluxSchnellTransformerConfig = #"{"guidance_embeds": false}"#
    static let sd1ModelIndex = #"{"_class_name": "StableDiffusionPipeline"}"#
    static let sdxlModelIndex = #"{"_class_name": "StableDiffusionXLPipeline"}"#
    static let cogVideoModelIndex = #"{"_class_name": "CogVideoXPipeline"}"#
    static let turboSchedulerConfig =
        #"{"_class_name": "EulerAncestralDiscreteScheduler", "timestep_spacing": "trailing"}"#
    static let sdxlBaseSchedulerConfig =
        #"{"_class_name": "EulerDiscreteScheduler", "timestep_spacing": "leading"}"#


    static func makeGGUF(at url: URL, bytes: Int, fill: UInt8 = 0x77) throws {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data(bytes: bytes, fill: fill).write(to: url)
    }

    @discardableResult
    static func makeGGUF(architecture: String, at dir: URL, name: String) throws -> URL {
        var builder = GGUFFixtureBuilder(keyValueCount: 3)
        builder.addUInt32(key: "general.alignment", value: 32)
        builder.addStringArray(key: "general.tags", values: ["fixture"])
        builder.addString(key: "general.architecture", value: architecture)
        let url = dir.appendingPathComponent(name)
        try FileManager.default.createDirectory(
            at: dir, withIntermediateDirectories: true)
        try builder.write(to: url)
        return url
    }

    static func ggufData(architecture: String) -> Data {
        var builder = GGUFFixtureBuilder(keyValueCount: 1)
        builder.addString(key: "general.architecture", value: architecture)
        var payload = builder.data
        payload.append(data(bytes: 64))
        return payload
    }
}

struct GGUFFixtureBuilder {
    var data = Data("GGUF".utf8)

    init(keyValueCount: Int) {
        append(UInt32(3))
        append(UInt64(0))
        append(UInt64(keyValueCount))
    }

    mutating func append<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
    }

    mutating func appendString(_ string: String) {
        append(UInt64(string.utf8.count))
        data.append(Data(string.utf8))
    }

    mutating func addString(key: String, value: String) {
        appendString(key)
        append(UInt32(8))
        appendString(value)
    }

    mutating func addUInt32(key: String, value: UInt32) {
        appendString(key)
        append(UInt32(4))
        append(value)
    }

    mutating func addUInt64(key: String, value: UInt64) {
        appendString(key)
        append(UInt32(10))
        append(value)
    }

    mutating func addStringArray(key: String, values: [String]) {
        appendString(key)
        append(UInt32(9))
        append(UInt32(8))
        append(UInt64(values.count))
        for value in values {
            appendString(value)
        }
    }

    func write(to url: URL, trailingBytes: Int = 64) throws {
        var payload = data
        payload.append(DiscoveryFixtures.data(bytes: trailingBytes))
        try payload.write(to: url)
    }
}
