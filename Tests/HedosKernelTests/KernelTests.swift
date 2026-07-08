import Foundation
import Testing

@testable import HedosKernel

@Test func kernelIsConstructibleAndVersioned() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    _ = Kernel(directory: dir)
    #expect(!Kernel.version.isEmpty)
}

@Test func discoverScansUserConfiguredHFRootAndResolvesMlxLm() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let customRoot = dir.appendingPathComponent("custom-hf")
    try DiscoveryFixtures.makeHFRepo(
        at: customRoot.appendingPathComponent("hub"),
        DiscoveryFixtures.HFRepo(
            org: "mlx-community", repo: "Tiny-Chat-4bit",
            files: [("model.safetensors", 512), ("tokenizer.json", 64)],
            configJSON:
                #"{"architectures": ["LlamaForCausalLM"], "model_type": "llama", "quantization": {"bits": 4}}"#
        ))

    let kernel = Kernel(directory: dir.appendingPathComponent("support"))
    try await kernel.addHFCacheRoot(customRoot.path)
    _ = try await kernel.discover()

    let records = try await kernel.shelf()
    let record = try #require(records.first { $0.name.contains("Tiny-Chat-4bit") })
    #expect(record.source.kind == .huggingfaceCache)
    #expect(record.runtime.id == "python:mlx-lm")
    #expect(record.runtime.tier == .managed)
    #expect(record.state == .ready)
}
