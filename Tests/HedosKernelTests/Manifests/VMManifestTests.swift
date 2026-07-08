import Foundation
import Testing

@testable import HedosKernel

private func parse(_ toml: String, directory: URL? = nil) throws -> RuntimeManifest {
    try RuntimeManifest.load(table: try TOMLLite.parse(toml), directory: directory)
}

private let validVMManifest = """
    id = "vm-speak-test"
    modalities = ["speech"]
    capabilities = ["speak"]
    execution = "sync"
    detect = { extension = "pth" }

    [vm]
    image = "docker.io/library/python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"
    setup = ["pip install --no-cache-dir soundfile==0.12.1"]

    [invoke]
    command = "python3 {resources}/main.py --model {model} --out {outputs} --text {prompt}"
    """

@Test func vmSectionParsesImageAndSetup() throws {
    let manifest = try parse(validVMManifest)
    #expect(manifest.vm?.image.contains("@sha256:") == true)
    #expect(manifest.vm?.setup.count == 1)
    #expect(manifest.invoke != nil)
}

@Test func vmImageMustBeDigestPinned() throws {
    let floating = validVMManifest.replacingOccurrences(
        of: "python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df",
        with: "python:3.12-alpine")
    #expect(throws: ManifestValidationError.self) { try parse(floating) }
}

@Test func vmRejectsServeAndEnvCombinations() throws {
    let withServe = validVMManifest.replacingOccurrences(
        of: "[invoke]\ncommand = \"python3 {resources}/main.py --model {model} --out {outputs} --text {prompt}\"",
        with: "[serve]\nentrypoint = \"main.py\"")
    #expect(throws: ManifestValidationError.self) { try parse(withServe) }

    let withEnv = validVMManifest + "\n[env]\nlockfile = \"requirements.lock\"\n"
    #expect(throws: ManifestValidationError.self) { try parse(withEnv) }
}

@Test func communityProvenanceRequiresVMSection() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let runtimeDir = dir.appendingPathComponent("plain-runtime", isDirectory: true)
    try FileManager.default.createDirectory(at: runtimeDir, withIntermediateDirectories: true)
    let plain = """
        id = "plain-runtime"
        capabilities = ["speak"]
        execution = "sync"
        detect = { extension = "pth" }

        [invoke]
        command = "echo {prompt}"
        """
    try plain.write(
        to: runtimeDir.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)
    try RuntimeProvenance(origin: RuntimeProvenance.communityOrigin).write(in: runtimeDir)

    let store = UserRuntimeStore(directory: dir)
    let (manifests, issues) = store.load(reservedIDs: [])
    #expect(manifests.isEmpty)
    #expect(issues.contains { $0.contains("run contained") })
}

@Test func userAuthoredManifestsLoadWithoutProvenance() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let runtimeDir = dir.appendingPathComponent("vm-speak-test", isDirectory: true)
    try FileManager.default.createDirectory(at: runtimeDir, withIntermediateDirectories: true)
    try validVMManifest.write(
        to: runtimeDir.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)

    let store = UserRuntimeStore(directory: dir)
    let (manifests, _) = store.load(reservedIDs: [])
    #expect(manifests.count == 1)
    #expect(manifests[0].provenance == nil)
    #expect(manifests[0].vm != nil)
}

@Test func installerPreviewCarriesThePermissionDiff() throws {
    let source = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: source) }
    try validVMManifest.write(
        to: source.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)
    let runtimes = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: runtimes) }

    let installer = ManifestInstaller(runtimesDirectory: runtimes, reservedIDs: ["ollama"])
    let preview = try installer.preview(
        from: source, vmAssetState: .absent(approxDownloadMB: 104))
    #expect(preview.id == "vm-speak-test")
    #expect(preview.capabilities == ["speak"])
    #expect(preview.image.contains("@sha256:"))
    #expect(preview.setup.count == 1)
    #expect(preview.vmAssetDownloadMB == 104)
    #expect(preview.detectSummary == ".pth files")

    let ready = try installer.preview(from: source, vmAssetState: .ready)
    #expect(ready.vmAssetDownloadMB == nil)
}

@Test func installerRoundTripWithProvenanceAndUninstall() throws {
    let source = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: source) }
    try validVMManifest.write(
        to: source.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)
    try Data("entry".utf8).write(to: source.appendingPathComponent("main.py"))
    let runtimes = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: runtimes) }

    let installer = ManifestInstaller(runtimesDirectory: runtimes, reservedIDs: [])
    let id = try installer.install(from: source)
    #expect(id == "vm-speak-test")
    let installedDir = runtimes.appendingPathComponent("vm-speak-test")
    #expect(FileManager.default.fileExists(atPath: installedDir.appendingPathComponent("main.py").path))
    #expect(RuntimeProvenance.read(in: installedDir)?.isCommunity == true)

    let store = UserRuntimeStore(directory: runtimes)
    let (manifests, issues) = store.load(reservedIDs: [])
    #expect(manifests.count == 1)
    #expect(manifests[0].provenance?.isCommunity == true)
    #expect(issues.isEmpty)

    #expect(throws: ManifestValidationError.self) { try installer.install(from: source) }

    try installer.uninstall(id: "vm-speak-test")
    #expect(!FileManager.default.fileExists(atPath: installedDir.path))
    #expect(throws: ManifestValidationError.self) {
        try installer.uninstall(id: "vm-speak-test")
    }
}

@Test func installerRefusesManifestsWithoutVM() throws {
    let source = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: source) }
    let plain = """
        id = "plain"
        capabilities = ["speak"]
        execution = "sync"
        detect = { extension = "pth" }

        [invoke]
        command = "echo hi"
        """
    try plain.write(
        to: source.appendingPathComponent("manifest.toml"), atomically: true, encoding: .utf8)
    let runtimes = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: runtimes) }
    let installer = ManifestInstaller(runtimesDirectory: runtimes, reservedIDs: [])
    #expect(throws: ManifestValidationError.self) {
        _ = try installer.preview(from: source, vmAssetState: .ready)
    }
    #expect(throws: ManifestValidationError.self) { _ = try installer.install(from: source) }
}
