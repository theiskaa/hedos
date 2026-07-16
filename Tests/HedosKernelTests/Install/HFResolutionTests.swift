import Foundation
import Synchronization
import Testing

@testable import HedosKernel

private final class MemorySecretStore: SecretStore, @unchecked Sendable {
    private let secrets = Mutex<[String: String]>([:])

    init(_ initial: [String: String] = [:]) {
        secrets.withLock { $0 = initial }
    }

    func set(_ secret: String, account: String) throws {
        secrets.withLock { $0[account] = secret }
    }

    func get(account: String) throws -> String? {
        secrets.withLock { $0[account] }
    }

    func delete(account: String) throws {
        secrets.withLock { $0[account] = nil }
    }
}

private func tempDirectory() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-hf-resolution-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

struct HFResolutionTests {
    @Test func rootPrefersHubCacheThenHomeThenSettingsThenDefault() async throws {
        let dir = try tempDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let home = URL(fileURLWithPath: "/Users/someone")
        let settings = SettingsStore(directory: dir)

        let explicit = HuggingFaceInstallProvider.defaultRoot(
            environment: ["HF_HUB_CACHE": "/caches/hub", "HF_HOME": "/caches/hf"],
            settings: settings, home: home)
        #expect(await explicit().path == "/caches/hub")

        let viaHome = HuggingFaceInstallProvider.defaultRoot(
            environment: ["HF_HOME": "/caches/hf"], settings: settings, home: home)
        #expect(await viaHome().path == "/caches/hf/hub")

        let fallback = HuggingFaceInstallProvider.defaultRoot(
            environment: [:], settings: settings, home: home)
        #expect(await fallback().path == "/Users/someone/.cache/huggingface/hub")

        var models = ModelsSettings()
        models.hfCacheRoots = ["/volumes/models/hf"]
        try await settings.save(models)
        let viaSettings = HuggingFaceInstallProvider.defaultRoot(
            environment: [:], settings: settings, home: home)
        #expect(await viaSettings().path == "/volumes/models/hf")
    }

    @Test func tokenPrefersKeychainThenEnvThenTokenFile() throws {
        let dir = try tempDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let tokenFile = dir.appendingPathComponent(".cache/huggingface/token")
        try FileManager.default.createDirectory(
            at: tokenFile.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("hf_from_file\n".utf8).write(to: tokenFile)

        let keychain = HuggingFaceInstallProvider.defaultToken(
            secrets: MemorySecretStore(["huggingface": "hf_from_keychain"]),
            environment: ["HF_TOKEN": "hf_from_env"], home: dir)
        #expect(keychain() == "hf_from_keychain")

        let env = HuggingFaceInstallProvider.defaultToken(
            secrets: MemorySecretStore(), environment: ["HF_TOKEN": "hf_from_env"], home: dir)
        #expect(env() == "hf_from_env")

        let file = HuggingFaceInstallProvider.defaultToken(
            secrets: MemorySecretStore(), environment: [:], home: dir)
        #expect(file() == "hf_from_file")
    }

    @Test func tokenFileHonorsHFHomeAndBlankMeansNone() throws {
        let dir = try tempDirectory()
        defer { try? FileManager.default.removeItem(at: dir) }
        let hfHome = dir.appendingPathComponent("hf-home")
        try FileManager.default.createDirectory(at: hfHome, withIntermediateDirectories: true)
        try Data("hf_scoped".utf8).write(to: hfHome.appendingPathComponent("token"))

        let scoped = HuggingFaceInstallProvider.defaultToken(
            secrets: MemorySecretStore(), environment: ["HF_HOME": hfHome.path], home: dir)
        #expect(scoped() == "hf_scoped")

        try Data("   \n".utf8).write(to: hfHome.appendingPathComponent("token"))
        #expect(scoped() == nil)

        let missing = HuggingFaceInstallProvider.defaultToken(
            secrets: MemorySecretStore(), environment: [:],
            home: dir.appendingPathComponent("nowhere"))
        #expect(missing() == nil)
    }
}
