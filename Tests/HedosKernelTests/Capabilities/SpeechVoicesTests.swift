import Foundation
import Testing

@testable import HedosKernel

private func speakerRecord(at path: String) -> ModelRecord {
    ModelRecord(
        name: "Kokoro-82M",
        modality: .speech,
        capabilities: [.speak],
        source: ModelSource(kind: .file, path: path))
}

private func makeVoices(_ names: [String], in directory: URL) throws {
    let voices = directory.appendingPathComponent("voices")
    try FileManager.default.createDirectory(at: voices, withIntermediateDirectories: true)
    for name in names {
        try Data().write(to: voices.appendingPathComponent(name))
    }
}

@Test func voicesAreFoundForSafetensorsBuilds() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try makeVoices(["af_heart.safetensors", "am_michael.safetensors"], in: dir)

    #expect(SpeechVoices.available(speakerRecord(at: dir.path)) == ["af_heart", "am_michael"])
}

@Test func voicesAreFoundForTorchBuilds() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try makeVoices(["am_michael.pt", "bf_emma.pt"], in: dir)

    #expect(SpeechVoices.available(speakerRecord(at: dir.path)) == ["am_michael", "bf_emma"])
}

@Test func voicesDeduplicateAcrossFormatsAndIgnoreStrayFiles() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try makeVoices(
        ["af_heart.safetensors", "af_heart.pt", "README.md", ".DS_Store", "notes.txt"], in: dir)

    #expect(SpeechVoices.available(speakerRecord(at: dir.path)) == ["af_heart"])
}

@Test func missingVoicesDirectoryYieldsNoVoices() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(SpeechVoices.available(speakerRecord(at: dir.path)).isEmpty)
}
