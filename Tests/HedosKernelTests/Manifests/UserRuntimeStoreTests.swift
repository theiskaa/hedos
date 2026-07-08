import Foundation
import Testing

@testable import HedosKernel

private func writeManifest(_ text: String, at directory: URL, name: String, loose: Bool) throws {
    if loose {
        try Data(text.utf8).write(to: directory.appendingPathComponent("\(name).toml"))
    } else {
        let subdir = directory.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: subdir, withIntermediateDirectories: true)
        try Data(text.utf8).write(to: subdir.appendingPathComponent("manifest.toml"))
    }
}

private let looseInvoke = """
    id = "%ID%"
    capabilities = ["chat"]
    execution = "stream"
    detect = { extension = "xyz" }
    [invoke]
    command = "echo {prompt}"
    """

@Test func absentDirectoryLoadsEmpty() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = UserRuntimeStore(directory: dir.appendingPathComponent("missing"))
    let result = store.load(reservedIDs: [])
    #expect(result.manifests.isEmpty)
    #expect(result.issues.isEmpty)
}

@Test func loadsDirectoryAndLooseManifests() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try writeManifest(
        looseInvoke.replacingOccurrences(of: "%ID%", with: "loose-one"),
        at: dir, name: "loose-one", loose: true)
    try writeManifest(
        looseInvoke.replacingOccurrences(of: "%ID%", with: "dir-one"),
        at: dir, name: "dir-one", loose: false)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: [])
    #expect(result.manifests.map(\.id).sorted() == ["dir-one", "loose-one"])
    let directoryBacked = result.manifests.first { $0.id == "dir-one" }
    #expect(directoryBacked?.directory != nil)
    #expect(result.issues.isEmpty)
}

@Test func brokenManifestBecomesNamedDiagnosticNotCrash() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try writeManifest("id = broken syntax here", at: dir, name: "bad", loose: true)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: [])
    #expect(result.manifests.isEmpty)
    #expect(result.issues.count == 1)
    #expect(result.issues.first?.contains("bad.toml") == true)
    #expect(result.issues.first?.contains("line") == true)
}

@Test func reservedIDIsSkippedWithDiagnostic() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try writeManifest(
        looseInvoke.replacingOccurrences(of: "%ID%", with: "ollama"),
        at: dir, name: "impostor", loose: true)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: ["ollama"])
    #expect(result.manifests.isEmpty)
    #expect(result.issues.first?.contains("reserved") == true)
}

@Test func duplicateUserIDKeepsFirstWithDiagnostic() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try writeManifest(
        looseInvoke.replacingOccurrences(of: "%ID%", with: "twin"),
        at: dir, name: "a-first", loose: true)
    try writeManifest(
        looseInvoke.replacingOccurrences(of: "%ID%", with: "twin"),
        at: dir, name: "b-second", loose: true)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: [])
    #expect(result.manifests.count == 1)
    #expect(result.issues.first?.contains("duplicate") == true)
}

@Test func looseServeManifestIsDiagnostic() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let serveManifest = """
        id = "loose-serve"
        capabilities = ["chat"]
        execution = "stream"
        detect = { extension = "xyz" }
        [serve]
        entrypoint = "main.py"
        """
    try writeManifest(serveManifest, at: dir, name: "loose-serve", loose: true)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: [])
    #expect(result.manifests.isEmpty)
    #expect(result.issues.first?.contains("directory") == true)
}

@Test func missingDetectLoadsWithWarningDiagnostic() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let noDetect = """
        id = "blind"
        capabilities = ["chat"]
        execution = "stream"
        [invoke]
        command = "echo hi"
        """
    try writeManifest(noDetect, at: dir, name: "blind", loose: true)

    let result = UserRuntimeStore(directory: dir).load(reservedIDs: [])
    #expect(result.manifests.count == 1)
    #expect(result.issues.first?.contains("never match") == true)
}
