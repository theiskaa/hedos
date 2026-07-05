import Foundation
import Testing

@testable import HedosKernel

@Test func settingsRoundTripAndDedup() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    #expect(try await store.load() == HedosSettings())

    _ = try await store.addWatchedFolder("/tmp/models-a")
    _ = try await store.addWatchedFolder("/tmp/models-a")
    let settings = try await store.addWatchedFolder("~/models-b")
    #expect(settings.watchedFolders.count == 2)
    #expect(settings.watchedFolders[1].hasSuffix("/models-b"))
    #expect(!settings.watchedFolders[1].contains("~"))

    let reloaded = try await SettingsStore(directory: dir).load()
    #expect(reloaded == settings)

    let afterRemove = try await store.removeWatchedFolder("/tmp/models-a")
    #expect(afterRemove.watchedFolders.count == 1)
    let reloadedAgain = try await SettingsStore(directory: dir).load()
    #expect(reloadedAgain.watchedFolders == afterRemove.watchedFolders)
}

@Test func watchedFolderFlowsIntoDiscovery() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let modelsDir = dir.appendingPathComponent("my-models")
    try DiscoveryFixtures.makeGGUF(
        at: modelsDir.appendingPathComponent("hidden-model.gguf"), bytes: 2048)

    let kernelDir = dir.appendingPathComponent("appsupport")
    let kernel = Kernel(directory: kernelDir, adapters: [])
    try await kernel.addWatchedFolder(modelsDir.path)
    #expect(try await kernel.watchedFolders() == [modelsDir.path])

    let scanner = LooseFileScanner(
        directories: LooseFileScanner.defaultDirectories()
            + (try await kernel.watchedFolders()).map {
                URL(fileURLWithPath: $0, isDirectory: true)
            })
    let result = await scanner.scan()
    #expect(result.discovered.contains { $0.name == "hidden-model" })

    try await kernel.removeWatchedFolder(modelsDir.path)
    #expect(try await kernel.watchedFolders().isEmpty)
}
