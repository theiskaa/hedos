import Foundation
import Testing

@testable import HedosKernel

private func vlmRecord() -> ModelRecord {
    ModelRecord(
        name: "Qwen2-VL-2B-Instruct-4bit",
        modality: .text,
        capabilities: [.chat, .see],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/models/hub/models--mlx-community--Qwen2-VL-2B-Instruct-4bit",
            repo: "mlx-community/Qwen2-VL-2B-Instruct-4bit"),
        runtime: RuntimeRef(id: "python:mlx-vlm", resolved: .auto, tier: .managed),
        execution: .stream, footprintMB: 900, state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private func embedRecord() -> ModelRecord {
    ModelRecord(
        name: "bge-small-en-v1.5",
        modality: .embedding,
        capabilities: [.embed],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/models/hub/models--BAAI--bge-small-en-v1.5",
            repo: "BAAI/bge-small-en-v1.5"),
        runtime: RuntimeRef(id: "python:embeddings", resolved: .auto, tier: .managed),
        execution: .stream, footprintMB: 130, state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func mlxVlmBidWinsSeeTaggedMlxRecords() {
    let adapter = MlxVlmAdapter()
    let record = vlmRecord()
    let vision = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.chat, .see],
        execution: .stream)
    let bid = adapter.bid(record, vision)
    #expect(bid?.tier == .managed)
    #expect(bid?.preference == 14)
    #expect(bid?.alternatives == [.mlxSwift])

    let textOnly = IdentifiedModel(
        format: .mlxSafetensors, modality: .text, capabilities: [.chat], execution: .stream)
    #expect(adapter.bid(record, textOnly) == nil)
    let ggufVision = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat, .see], execution: .stream)
    #expect(adapter.bid(record, ggufVision) == nil)
}

@Test func mlxVlmCanServeSeeAndChat() {
    let adapter = MlxVlmAdapter()
    let record = vlmRecord()
    #expect(adapter.canServe(record, .see))
    #expect(adapter.canServe(record, .chat))
    #expect(adapter.canServe(record, .complete))
    #expect(!adapter.canServe(record, .embed))
    #expect(!adapter.canServe(embedRecord(), .see))
}

@Test func embeddingsBidIsUniqueOnTheLadder() {
    let adapter = EmbeddingsAdapter()
    let record = embedRecord()
    for format in [ModelFormat.safetensors, .mlxSafetensors] {
        let identified = IdentifiedModel(
            format: format, modality: .embedding, capabilities: [.embed], execution: .stream)
        let bid = adapter.bid(record, identified)
        #expect(bid?.tier == .managed)
        #expect(bid?.preference == 32)
    }
    let chatModel = IdentifiedModel(
        format: .safetensors, modality: .text, capabilities: [.chat], execution: .stream)
    #expect(adapter.bid(record, chatModel) == nil)
    let ggufEmbed = IdentifiedModel(
        format: .gguf, modality: .embedding, capabilities: [.embed], execution: .stream)
    #expect(adapter.bid(record, ggufEmbed) == nil)
}

@Test func embeddingsCanServeOnlyEmbed() {
    let adapter = EmbeddingsAdapter()
    let record = embedRecord()
    #expect(adapter.canServe(record, .embed))
    #expect(!adapter.canServe(record, .chat))
    #expect(!adapter.canServe(vlmRecord(), .embed))
}

@Test func vlmAndEmbeddingsBundlesShipComplete() throws {
    for name in ["python-mlx-vlm", "python-embeddings"] {
        let bundle = try #require(RuntimeBundle.directory(named: name))
        for file in ["manifest.toml", "requirements.in", "requirements.lock", "sandbox.sb", "main.py"] {
            #expect(
                FileManager.default.fileExists(
                    atPath: bundle.appendingPathComponent(file).path),
                "\(name) is missing \(file)")
        }
        let lock = try String(
            contentsOf: bundle.appendingPathComponent("requirements.lock"), encoding: .utf8)
        #expect(lock.contains("--hash=sha256:"))
    }
}
