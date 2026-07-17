import Foundation
import Testing

@testable import HedosKernel

@Test func corruptSettingsFileIsQuarantinedAndDefaultsReturned() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let settingsDir = dir.appendingPathComponent("settings", isDirectory: true)
    try FileManager.default.createDirectory(at: settingsDir, withIntermediateDirectories: true)
    let file = settingsDir.appendingPathComponent("\(GeneralSettings.domainName).json")
    try Data("not json {{{".utf8).write(to: file)

    let store = SettingsStore(directory: dir)
    let loaded = await store.general()
    #expect(loaded == GeneralSettings())

    let siblings = try FileManager.default.contentsOfDirectory(
        at: settingsDir, includingPropertiesForKeys: nil)
    #expect(!FileManager.default.fileExists(atPath: file.path))
    #expect(siblings.contains { $0.lastPathComponent.contains(".corrupt-") })
}

@Test func storeDecoderReadsBothPlainAndFractionalISO8601Dates() throws {
    struct Dated: Codable, Equatable {
        var at: Date
    }
    let decoder = StoreCoding.decoder()
    let plain = try decoder.decode(Dated.self, from: Data(#"{"at":"2026-07-10T06:00:00Z"}"#.utf8))
    let fractional = try decoder.decode(
        Dated.self, from: Data(#"{"at":"2026-07-10T06:00:00.250Z"}"#.utf8))
    #expect(plain.at == Date(timeIntervalSince1970: 1_783_663_200))
    #expect(fractional.at.timeIntervalSince(plain.at) == 0.25)

    let encoded = try StoreCoding.encoder().encode(fractional)
    let roundTripped = try decoder.decode(Dated.self, from: encoded)
    #expect(roundTripped == fractional)
}

@Test func directSettingsSaveAppliesStoredPoliciesThroughTheChangeStream() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(
        directory: dir, adapters: [], governor: MemoryGovernor(totalMemoryMB: 262_144),
        secrets: InMemorySecretStore())

    var settings = await kernel.settings.advanced()
    settings.jobHistoryLimit = 7
    try await kernel.settings.save(settings)

    var applied = false
    for _ in 0..<100 {
        if await kernel.scheduler.history.limit == 7 {
            applied = true
            break
        }
        try await Task.sleep(for: .milliseconds(20))
    }
    #expect(applied)
}
