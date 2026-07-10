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
            try data(bytes: tag.modelBytes, fill: UInt8(0x10 + index))
                .write(to: blobs.appendingPathComponent("sha256-\(digest)"))

            var layers: [[String: Any]] = [
                [
                    "mediaType": "application/vnd.ollama.image.model",
                    "size": tag.modelBytes,
                    "digest": "sha256:\(digest)",
                ]
            ]
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
        var writeRefsMain = true
        var revision = "abc123def456"
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
    }

    static let kokoroConfig =
        #"{"istftnet": {}, "plbert": {}, "style_dim": 128, "n_mels": 80, "n_token": 178}"#
    static let causalLMConfig =
        #"{"architectures": ["Qwen3ForCausalLM"], "model_type": "qwen3"}"#
    static let fluxModelIndex = #"{"_class_name": "FluxPipeline"}"#
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
