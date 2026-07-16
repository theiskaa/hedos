import Foundation
import Testing

@testable import HedosKernel

@Test func ollamaLiveDeleteRemovesModelFromDaemonAndShelf() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_OLLAMA_LIVE"] != nil else { return }
    guard OllamaAdapter.daemonBinary() != nil else { return }

    let reference = ProcessInfo.processInfo.environment["HEDOS_OLLAMA_PULL"] ?? "all-minilm:22m"
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-ollama-live-delete-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: directory) }

    let kernel = Kernel(
        directory: directory, governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore())
    let plan = try await kernel.installs.plan(provider: .ollama, reference: reference)
    let id = try await kernel.installs.begin(plan)
    for await event in await kernel.installs.events(id: id) {
        if case .failed(let message) = event {
            Issue.record("live pull failed: \(message)")
            return
        }
    }
    _ = try await kernel.discover()
    guard
        let record = try await kernel.shelf().first(where: {
            $0.source.kind == .ollama && $0.name == plan.reference
        })
    else {
        Issue.record("pulled model never landed on the shelf")
        return
    }

    let report = try await kernel.deleteModel(record.id)

    #expect(report.daemonDeleted)
    #expect(try await kernel.registry.get(id: record.id) == nil)
    _ = try await kernel.discover()
    let survivors = try await kernel.shelf().filter {
        $0.source.kind == .ollama && $0.name == plan.reference
    }
    #expect(survivors.isEmpty)
}
