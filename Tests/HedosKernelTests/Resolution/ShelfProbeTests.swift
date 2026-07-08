import Foundation
import Testing

@testable import HedosKernel

private func probeFixtures(in dir: URL) throws -> (gguf: ModelRecord, flux: ModelRecord, unknown: ModelRecord) {
    let ggufPath = dir.appendingPathComponent("tiny.gguf")
    var payload = Data("GGUF".utf8)
    payload.append(DiscoveryFixtures.data(bytes: 64))
    try payload.write(to: ggufPath)
    let gguf = ModelRecord(
        name: "tiny", modality: .text, capabilities: [.chat, .complete],
        source: ModelSource(kind: .file, path: ggufPath.path),
        execution: .stream)

    try DiscoveryFixtures.makeHFRepo(
        at: dir,
        DiscoveryFixtures.HFRepo(
            org: "acme", repo: "flux-schnell",
            files: [("weights.safetensors", 64)],
            modelIndexJSON: DiscoveryFixtures.fluxModelIndex))
    let flux = ModelRecord(
        name: "flux-schnell", modality: .unknown, capabilities: [],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: dir.appendingPathComponent("models--acme--flux-schnell").path,
            repo: "acme/flux-schnell",
            ref: "abc123def456"))

    let bundle = dir.appendingPathComponent("mystery")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(#"{"_class_name": "SomePipeline"}"#.utf8)
        .write(to: bundle.appendingPathComponent("model_index.json"))
    let unknown = ModelRecord(
        name: "mystery", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))

    return (gguf, flux, unknown)
}

@Test func explainReportsBidsAndWinner() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let fixtures = try probeFixtures(in: dir)

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(fixtures.gguf)
    try await registry.register(fixtures.flux)
    try await registry.register(fixtures.unknown)

    let engine = ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter(), MfluxAdapter()])
    let explanations = try await engine.explainAll(in: registry)
    let byID = Dictionary(uniqueKeysWithValues: explanations.map { ($0.record.id, $0) })

    let gguf = try #require(byID[fixtures.gguf.id])
    #expect(gguf.winner == "llama-cpp")
    #expect(!gguf.bids.isEmpty)

    let flux = try #require(byID[fixtures.flux.id])
    #expect(flux.winner == "python:mflux")
    #expect(flux.bids.count == 1)
    #expect(flux.identified.pipelineClass == "FluxPipeline")

    let unknown = try #require(byID[fixtures.unknown.id])
    #expect(unknown.winner == nil)
    #expect(unknown.bids.isEmpty)
}

@Test func shelfReportRendersHonestNoBidReasons() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let fixtures = try probeFixtures(in: dir)

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(fixtures.gguf)
    try await registry.register(fixtures.flux)
    try await registry.register(fixtures.unknown)

    let engine = ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter(), MfluxAdapter()])
    let report = ShelfReport.render(try await engine.explainAll(in: registry))

    #expect(report.contains("llama-cpp (native)"))
    #expect(report.contains("python:mflux (managed)"))
    #expect(report.contains("SomePipeline"))
    #expect(report.contains("unrecognized diffusers pipeline class"))

    let identifyOnly = ResolutionExplanation(
        record: fixtures.unknown,
        identified: IdentifiedModel(
            format: .diffusers, modality: .video, capabilities: [], execution: .job,
            params: [], pipelineClass: "CogVideoXPipeline"),
        bids: [])
    #expect(ShelfReport.line(identifyOnly).contains("not runnable"))
}
