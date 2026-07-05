import Foundation

@testable import HedosKernel

/// Builds miniature on-disk store layouts mirroring the real formats the
/// scanners were designed against (verified Ollama manifests, HF hub cache
/// with refs/snapshots/blobs indirection).
enum DiscoveryFixtures {
    static func data(bytes: Int, fill: UInt8 = 0xAB) -> Data {
        Data(repeating: fill, count: bytes)
    }

    // MARK: - Ollama

    struct OllamaTag {
        var namespace = "library"
        var model: String
        var tag: String
        var modelBytes: Int
        var extraBytes: Int = 0
        var malformed = false
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
            if tag.extraBytes > 0 {
                layers.append([
                    "mediaType": "application/vnd.ollama.image.params",
                    "size": tag.extraBytes,
                    "digest": "sha256:\(String(format: "%064d", 900 + index))",
                ])
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

    // MARK: - Hugging Face cache

    struct HFRepo {
        var org: String
        var repo: String
        var files: [(name: String, bytes: Int)]
        var configJSON: String?
        var modelIndexJSON: String?
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
    }

    /// Mirrors the real Kokoro-82M config shape: no architectures[] key,
    /// StyleTTS2-signature keys instead (verified against the actual repo).
    static let kokoroConfig =
        #"{"istftnet": {}, "plbert": {}, "style_dim": 128, "n_mels": 80, "n_token": 178}"#
    static let causalLMConfig =
        #"{"architectures": ["Qwen3ForCausalLM"], "model_type": "qwen3"}"#
    static let fluxModelIndex = #"{"_class_name": "FluxPipeline"}"#

    // MARK: - LM Studio / loose files

    static func makeGGUF(at url: URL, bytes: Int, fill: UInt8 = 0x77) throws {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data(bytes: bytes, fill: fill).write(to: url)
    }
}
