import Foundation
import Testing

@testable import HedosKernel

@Test func huggingFaceLiveInstallLandsInScannableCache() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_HF_LIVE"] != nil else { return }

    let repo =
        ProcessInfo.processInfo.environment["HEDOS_HF_REPO"]
        ?? "hf-internal-testing/tiny-random-gpt2"
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-hf-live-\(UUID().uuidString)")
        .appendingPathComponent("hub")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }

    let provider = HuggingFaceInstallProvider(root: root)
    let service = InstallService(providers: [provider])
    let plan = try await service.plan(provider: .huggingface, reference: repo)
    #expect(plan.totalBytes ?? 0 > 0)
    let id = try await service.begin(plan)
    var sawProgress = false
    var terminal: InstallEvent?
    for await event in await service.events(id: id) {
        if case .progress = event { sawProgress = true }
        if event.isTerminal { terminal = event }
    }
    #expect(terminal == .done)
    #expect(sawProgress)

    let result = await HFCacheScanner(root: root).scan()
    let model = result.discovered.first { $0.source.repo == repo }
    #expect(model != nil)
    #expect(model?.downloading == false)
}
