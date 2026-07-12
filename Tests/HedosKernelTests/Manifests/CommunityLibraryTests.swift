import Foundation
import Testing

@testable import HedosKernel

private func folderRecord(at directory: URL, name: String) -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .speech,
        capabilities: [.speak],
        source: ModelSource(kind: .folder, path: directory.path),
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

@Test func everySeededManifestIsValidAndContained() throws {
    let recipes = CommunityLibrary().recipes()
    #expect(!recipes.isEmpty)
    var ids = Set<String>()
    let reserved: Set<String> = [
        RuntimeID.llamaCpp, .whisperCpp, .ollama, .mlxSwift, .appleFoundation, .openAIEndpoint,
        .mflux, .diffusers, .mlxLm, .mlxAudio, .mlxVlm, .embeddings, .comfyUI, .a1111,
    ].reduce(into: Set<String>()) { $0.insert($1.rawValue) }
    for recipe in recipes {
        let manifest = recipe.manifest
        #expect(!ids.contains(manifest.id), "duplicate id \(manifest.id)")
        ids.insert(manifest.id)
        #expect(!reserved.contains(manifest.id), "\(manifest.id) collides with a built-in")
        let vm = try #require(manifest.vm, "\(manifest.id) needs a [vm]")
        #expect(vm.image.contains("@sha256:"), "\(manifest.id) image is not digest-pinned")
        #expect(manifest.detect != nil, "\(manifest.id) has no detect rule")
    }
}

@Test func seededLibraryIncludesTheKokoroProof() {
    let ids = CommunityLibrary().recipes().map(\.manifest.id)
    #expect(ids.contains("python:kokoro-vm"))
}

@Test func communityLibraryMatchesADetectedFixture() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data("weights".utf8).write(to: dir.appendingPathComponent("kokoro-v1_0.pth"))
    let record = folderRecord(at: dir, name: "kokoro")

    let matches = CommunityLibrary().matches(record: record)
    #expect(matches.map(\.manifest.id) == ["python:kokoro-vm"])
}

@Test func communityLibraryDoesNotMatchAnUnrelatedRecord() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data("x".utf8).write(to: dir.appendingPathComponent("random.bin"))
    let record = folderRecord(at: dir, name: "mystery")
    #expect(CommunityLibrary().matches(record: record).isEmpty)
}

@Test func seededManifestPreviewsInstallCleanly() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let installer = ManifestInstaller(
        runtimesDirectory: dir.appendingPathComponent("runtimes.d"), reservedIDs: [])
    for recipe in CommunityLibrary().recipes() {
        let preview = try installer.preview(from: recipe.directory, vmAssetState: .ready)
        #expect(preview.id == recipe.manifest.id)
        #expect(preview.image.contains("@sha256:"))
    }
}
