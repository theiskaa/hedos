import Foundation
import Testing

@testable import HedosKernel

private func sineWave(samples: Int, amplitude: Float = 0.5) -> Data {
    var values = [Float]()
    values.reserveCapacity(samples)
    for index in 0..<samples {
        values.append(amplitude * sin(Float(index) * 0.1))
    }
    return values.withUnsafeBytes { Data($0) }
}

@Test func wavEncodingProducesValidHeaderAndSampleCount() {
    let pcm = sineWave(samples: 2400)
    let wav = SpeechAudio.wavData(fromFloat32: pcm, sampleRate: 24000)

    #expect(String(data: wav.prefix(4), encoding: .ascii) == "RIFF")
    #expect(String(data: wav.subdata(in: 8..<12), encoding: .ascii) == "WAVE")
    #expect(String(data: wav.subdata(in: 12..<16), encoding: .ascii) == "fmt ")
    #expect(String(data: wav.subdata(in: 36..<40), encoding: .ascii) == "data")
    #expect(wav.count == 44 + 2400 * 2)

    let dataSize = wav.subdata(in: 40..<44).withUnsafeBytes {
        $0.load(as: UInt32.self).littleEndian
    }
    #expect(dataSize == 2400 * 2)
    let rate = wav.subdata(in: 24..<28).withUnsafeBytes {
        $0.load(as: UInt32.self).littleEndian
    }
    #expect(rate == 24000)
}

@Test func peaksNormalizeAndBucketize() {
    let pcm = sineWave(samples: 28_000, amplitude: 0.3)
    let peaks = SpeechAudio.peaks(fromFloat32: pcm)

    #expect(peaks.count == 28)
    #expect(peaks.max() == 1.0)
    #expect(peaks.allSatisfy { $0 >= 0 && $0 <= 1 })

    let silence = Data(count: 4800 * 4)
    let flat = SpeechAudio.peaks(fromFloat32: silence)
    #expect(flat.count == 28)
    #expect(flat.allSatisfy { $0 == 0 })
}

@Test func durationDerivesFromSampleCount() {
    let pcm = sineWave(samples: 24000)
    #expect(SpeechAudio.durationMs(fromFloat32: pcm, sampleRate: 24000) == 1000)
    #expect(SpeechAudio.durationMs(fromFloat32: pcm, sampleRate: 0) == 0)
}

@Test func saveSpeechStoresWavArtifactWithProvenance() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    var record = Fixtures.gguf(path: "~/models/reader.gguf")
    record.modality = .speech
    record.capabilities = [.speak]
    record.runtime = RuntimeRef(id: "python:mlx-audio", resolved: .auto, tier: .managed)
    record.state = .ready
    try await kernel.registry.register(record)

    let pcm = sineWave(samples: 4800)
    let artifact = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "hello there",
        sampleRate: 24000, pcm: pcm)

    #expect(artifact.capability == .speak)
    #expect(artifact.durationMs == 200)
    guard case .object(let fields) = artifact.params else {
        Issue.record("params not an object")
        return
    }
    #expect(fields["text"] == .string("hello there"))
    #expect(fields["voice"] == .string("af_heart"))
    if case .array(let peaks)? = fields["peaks"] {
        #expect(peaks.count == 28)
    } else {
        Issue.record("peaks missing")
    }

    let listed = try await kernel.artifactStore.list()
    #expect(listed.contains { $0.id == artifact.id })
    let url = try await kernel.artifactStore.url(id: artifact.id)
    let header = try Data(contentsOf: #require(url)).prefix(4)
    #expect(String(data: header, encoding: .ascii) == "RIFF")
}

@Test func saveSpeechRemembersOwningSessionAndLegacySidecarsStillDecode() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, record) = try await speakingKernel(in: dir)

    let session = try await kernel.chats.createSession(modelID: record.id)
    let spoken = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "narrate this",
        sampleRate: 24000, pcm: sineWave(samples: 2400), sessionID: session.id)
    #expect(spoken.sessionID == session.id)

    let orphan = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "no session",
        sampleRate: 24000, pcm: sineWave(samples: 2400))
    #expect(orphan.sessionID == nil)

    let reread = try await kernel.artifactStore.list()
    #expect(reread.first { $0.id == spoken.id }?.sessionID == session.id)

    let sidecar = dir.appendingPathComponent("outputs")
    let years = try FileManager.default.contentsOfDirectory(at: sidecar, includingPropertiesForKeys: nil)
    let legacy = try #require(
        years.flatMap {
            (try? FileManager.default.contentsOfDirectory(at: $0, includingPropertiesForKeys: nil)) ?? []
        }.first { $0.pathExtension == "json" })
    var fields = try #require(
        try JSONSerialization.jsonObject(with: Data(contentsOf: legacy)) as? [String: Any])
    fields.removeValue(forKey: "sessionID")
    try JSONSerialization.data(withJSONObject: fields).write(to: legacy)

    let fresh = Kernel(directory: dir, adapters: [])
    let afterLegacy = try await fresh.artifactStore.list()
    #expect(afterLegacy.count == 2)
    #expect(afterLegacy.contains { $0.sessionID == nil })
}

@Test func artifactOwnersDropsArtifactsWhoseConversationWasDeleted() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, record) = try await speakingKernel(in: dir)

    let session = try await kernel.chats.createSession(modelID: record.id)
    let spoken = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "narrate this",
        sampleRate: 24000, pcm: sineWave(samples: 2400), sessionID: session.id)
    try await kernel.chats.appendGeneratedTurn(
        prompt: "narrate this", artifactID: spoken.id,
        capabilityTag: SessionTag.spoke, to: session.id)

    #expect(try await kernel.chats.artifactOwners()[spoken.id] == session.id)

    try await kernel.chats.deleteSession(id: session.id)

    #expect(try await kernel.chats.artifactOwners().isEmpty)
    #expect(try await kernel.artifactStore.list().contains { $0.id == spoken.id })
}

private func speakingKernel(in directory: URL) async throws -> (Kernel, ModelRecord) {
    let kernel = Kernel(directory: directory, adapters: [])
    var record = Fixtures.gguf(path: "~/models/reader.gguf")
    record.modality = .speech
    record.capabilities = [.speak]
    record.runtime = RuntimeRef(id: "python:mlx-audio", resolved: .auto, tier: .managed)
    record.state = .ready
    try await kernel.registry.register(record)
    return (kernel, record)
}

@Test func replaceSpokenArtifactSwapsRefAndTagsSessionSpoke() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, record) = try await speakingKernel(in: dir)

    let session = try await kernel.chats.createSession(modelID: "chatty")
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .user, content: "say something"), to: session.id)
    let answer = try await kernel.chats.appendTurn(
        TurnDraft(role: .assistant, content: "something spoken", modelID: "chatty"),
        to: session.id)

    let first = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "something spoken",
        sampleRate: 24000, pcm: sineWave(samples: 2400))
    try await kernel.replaceSpokenArtifact(
        sessionID: session.id, turnID: answer.id, artifactID: first.id)

    let once = try #require(try await kernel.chats.session(id: session.id))
    #expect(once.turns.first { $0.id == answer.id }?.artifactRefs == [first.id])
    #expect(once.session.capabilityTags.contains(SessionTag.spoke))

    let second = try await kernel.saveSpeech(
        modelID: record.id, voice: "am_michael", text: "something spoken",
        sampleRate: 24000, pcm: sineWave(samples: 4800))
    try await kernel.replaceSpokenArtifact(
        sessionID: session.id, turnID: answer.id, artifactID: second.id)

    let reloaded = try #require(try await kernel.chats.session(id: session.id))
    let turn = try #require(reloaded.turns.first { $0.id == answer.id })
    #expect(turn.artifactRefs == [second.id])
    #expect(try await kernel.artifactStore.get(id: first.id) == nil)
    #expect(try await kernel.artifactStore.get(id: second.id) != nil)
    #expect(try await kernel.artifactStore.list().count == 1)
}

@Test func replaceSpokenArtifactIsIdempotentAndKeepsOtherArtifacts() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, record) = try await speakingKernel(in: dir)

    let session = try await kernel.chats.createSession(modelID: "chatty")
    let answer = try await kernel.chats.appendTurn(
        TurnDraft(role: .assistant, content: "spoken", artifactRefs: ["image-ref"]),
        to: session.id)

    let spoken = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "spoken",
        sampleRate: 24000, pcm: sineWave(samples: 2400))
    try await kernel.replaceSpokenArtifact(
        sessionID: session.id, turnID: answer.id, artifactID: spoken.id)
    try await kernel.replaceSpokenArtifact(
        sessionID: session.id, turnID: answer.id, artifactID: spoken.id)

    let reloaded = try #require(try await kernel.chats.session(id: session.id))
    let turn = try #require(reloaded.turns.first { $0.id == answer.id })
    #expect(turn.artifactRefs == ["image-ref", spoken.id])
    #expect(try await kernel.artifactStore.get(id: spoken.id) != nil)
}

@Test func replaceSpokenArtifactRejectsUnknownTurn() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let (kernel, record) = try await speakingKernel(in: dir)
    let session = try await kernel.chats.createSession(modelID: "chatty")
    let artifact = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "x",
        sampleRate: 24000, pcm: sineWave(samples: 2400))

    await #expect(throws: (any Error).self) {
        try await kernel.replaceSpokenArtifact(
            sessionID: session.id, turnID: "missing", artifactID: artifact.id)
    }
}

@Test func speakableTextStripsMarkdownForTheEar() {
    let markdown = """
        ## Heading

        Some **bold** and _italic_ with `inline code` and a [link](https://x.y).

        ```swift
        let hidden = true
        ```

        - first item
        - second item

        | a | b |
        | - | - |

        > quoted wisdom
        """
    let spoken = SpeechText.speakable(markdown)
    #expect(!spoken.contains("#"))
    #expect(!spoken.contains("*"))
    #expect(!spoken.contains("`"))
    #expect(!spoken.contains("hidden"))
    #expect(!spoken.contains("|"))
    #expect(spoken.contains("Heading"))
    #expect(spoken.contains("Some bold and italic with inline code and a link."))
    #expect(spoken.contains("first item"))
    #expect(spoken.contains("quoted wisdom"))
    #expect(SpeechText.speakable("") == "")
}
