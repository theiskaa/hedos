import Foundation
import Testing

@testable import HedosKernel

@Test func mlxSwiftLiveGeneratesRealTokens() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_MLX_LIVE"] != nil else { return }

    let path =
        ProcessInfo.processInfo.environment["HEDOS_MLX_MODEL"]
        ?? NSString(
            string:
                "~/models/huggingface/hub/models--mlx-community--Llama-3.2-1B-Instruct-4bit/snapshots/08231374eeacb049a0eade7922910865b8fce912"
        ).expandingTildeInPath

    var isDirectory: ObjCBool = false
    guard FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory),
        isDirectory.boolValue
    else { return }

    let governor = MemoryGovernor()
    let footprintMB = MlxSwiftEngine.directoryFootprintMB(path: path)

    var chunks: [CapabilityChunk] = []
    let stream = AsyncThrowingStream<CapabilityChunk, Error> { continuation in
        let task = Task {
            await MlxSwiftEngine.shared.run(
                path: path,
                modelID: "live-mlx",
                modelName: "Llama-3.2-1B",
                footprintMB: footprintMB,
                governor: governor,
                messages: [ChatMessage(role: .user, content: "Say hello in one short sentence.")],
                params: .init(maxTokens: 32),
                continuation: continuation)
        }
        continuation.onTermination = { _ in task.cancel() }
    }

    for try await chunk in stream {
        chunks.append(chunk)
    }

    let hasNonEmptyText = chunks.contains { chunk in
        if case .text(let value) = chunk { return !value.isEmpty }
        return false
    }
    #expect(hasNonEmptyText)

    let stats = chunks.compactMap { chunk -> GenerationStats? in
        if case .done(let stats) = chunk { return stats }
        return nil
    }.first

    let doneStats = try #require(stats)
    #expect((doneStats.completionTokens ?? 0) > 0)
    #expect((doneStats.promptTokens ?? 0) > 0)
}
