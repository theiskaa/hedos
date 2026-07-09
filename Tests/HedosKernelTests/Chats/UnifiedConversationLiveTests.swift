import Foundation
import Testing

@testable import HedosKernel

@Test func unifiedConversationInterleavesTextVoiceAndImageLive() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_CONVERSATION_LIVE"] != nil else { return }

    let directory = URL(fileURLWithPath: NSHomeDirectory())
        .appendingPathComponent("Library/Application Support/Hedos")
    let kernel = Kernel(directory: directory)
    let shelf = try await kernel.shelf()

    func ready(_ capability: Capability, servedBy runtimes: [String]) -> ModelRecord? {
        for runtime in runtimes {
            if let match = shelf.first(where: {
                $0.state == .ready && $0.capabilities.contains(capability)
                    && $0.runtime.id == runtime
            }) {
                return match
            }
        }
        return nil
    }
    guard let speaker = ready(.speak, servedBy: ["python:mlx-audio"]),
        let painter = ready(.image, servedBy: ["python:diffusers", "python:mflux"])
    else { return }

    let line = "Hedos speaks inside the conversation."
    let prompt = "a koala riding a bicycle, flat vector"

    func sweepLeftovers() async throws {
        for artifact in (try? await kernel.artifacts()) ?? [] {
            guard case .object(let fields) = artifact.params else { continue }
            let text = fields["text"].flatMap { if case .string(let v) = $0 { return v } else { return nil } }
            let made = fields["prompt"].flatMap { if case .string(let v) = $0 { return v } else { return nil } }
            if text == line || made == prompt {
                try? await kernel.deleteArtifact(id: artifact.id)
            }
        }
    }
    try await sweepLeftovers()

    let session = try await kernel.chats.createSession(modelID: nil)
    let voice = (try? await kernel.voices(speaker.id))?.first ?? "af_heart"
    var pcm = Data()
    var sampleRate = 24000
    let speech = try await kernel.invoke(
        speaker.id, .speak,
        payload: .object(["text": .string(line), "voice": .string(voice)]))
    for try await chunk in speech {
        if case .audio(let frame) = chunk {
            sampleRate = frame.sampleRate
            pcm.append(frame.data)
        }
    }
    #expect(!pcm.isEmpty)
    let wav = try await kernel.saveSpeech(
        modelID: speaker.id, voice: voice, text: line, sampleRate: sampleRate, pcm: pcm)
    try await kernel.recordGeneratedTurn(
        sessionID: session.id, prompt: line, artifactID: wav.id, tag: SessionTag.spoke)

    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .user, content: "and now a picture"), to: session.id)
    _ = try await kernel.chats.appendTurn(
        TurnDraft(role: .assistant, content: "Sure — here you go."), to: session.id)

    let form = ParamForm(schema: painter.params)
    let jobID = try await kernel.submit(painter.id, .image, payload: form.payload(prompt: prompt))
    var produced: [String] = []
    for await event in await kernel.jobEvents(id: jobID) {
        if case .done(let result) = event { produced = result }
        if case .failed(let message) = event { Issue.record("image job failed: \(message)") }
    }
    let imageID = try #require(produced.first)
    try await kernel.recordGeneratedTurn(
        sessionID: session.id, prompt: prompt, artifactID: imageID,
        tag: SessionTag.generatedImage)

    let transcript = try #require(try await kernel.chats.session(id: session.id))
    #expect(transcript.turns.count == 6)
    #expect(transcript.turns[0].content == line)
    #expect(transcript.turns[1].artifactRefs == [wav.id])
    #expect(transcript.turns[3].content == "Sure — here you go.")
    #expect(transcript.turns[4].content == prompt)
    #expect(transcript.turns[5].artifactRefs == [imageID])
    #expect(
        Set(transcript.session.capabilityTags)
            == Set([SessionTag.spoke, SessionTag.generatedImage]))

    let spoken = try #require(try await kernel.artifact(id: wav.id))
    let drawn = try #require(try await kernel.artifact(id: imageID))
    #expect(spoken.capability == .speak)
    #expect(drawn.capability == .image)

    try await kernel.chats.deleteSession(id: session.id)
    try await kernel.deleteArtifact(id: wav.id)
    try await kernel.deleteArtifact(id: imageID)
}
