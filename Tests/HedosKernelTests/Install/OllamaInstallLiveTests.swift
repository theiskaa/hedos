import Foundation
import Testing

@testable import HedosKernel

@Test func ollamaLivePullLandsModelOnShelf() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_OLLAMA_LIVE"] != nil else { return }
    guard OllamaAdapter.daemonBinary() != nil else { return }

    let reference = ProcessInfo.processInfo.environment["HEDOS_OLLAMA_PULL"] ?? "all-minilm:22m"
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-ollama-live-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: directory) }

    let provider = OllamaInstallProvider()
    let service = InstallService(providers: [provider])
    let plan = try await service.plan(provider: .ollama, reference: reference)
    let id = try await service.begin(plan)
    var sawProgress = false
    var terminal: InstallEvent?
    for await event in await service.events(id: id) {
        if case .progress = event { sawProgress = true }
        if event.isTerminal { terminal = event }
    }
    #expect(terminal == .done)
    #expect(sawProgress)

    let kernel = Kernel(directory: directory, governor: MemoryGovernor())
    let summary = try await kernel.discover()
    #expect(summary.perKind[.ollama]?.count ?? 0 > 0)
    let records = try await kernel.registry.list()
    #expect(records.contains { $0.source.kind == .ollama && $0.name == reference })
}
