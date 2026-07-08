import Foundation
import Testing

@testable import HedosKernel

private func model(
    _ suffix: String, name: String, capabilities: [Capability], state: ModelState = .ready
) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(suffix).gguf")
    record.name = name
    record.capabilities = capabilities
    record.state = state
    return record
}

private func stage(_ record: ModelRecord, _ capability: Capability) -> PipelineStage {
    PipelineStage(modelID: record.id, capability: capability)
}

@Test func validatesGoodTranscribeChatSpeakChain() throws {
    let asr = model("asr", name: "whisper", capabilities: [.transcribe])
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    let tts = model("tts", name: "kokoro", capabilities: [.speak])
    let shelf = [asr, chat, tts]
    let signature = try PipelineValidator.validate(
        [stage(asr, .transcribe), stage(chat, .chat), stage(tts, .speak)], shelf: shelf)
    #expect(signature == PipelineSignature(input: .audio, output: .audio))
}

@Test func rejectsIncompatibleEdge() throws {
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    let asr = model("asr", name: "whisper", capabilities: [.transcribe])
    do {
        _ = try PipelineValidator.validate(
            [stage(chat, .chat), stage(asr, .transcribe)], shelf: [chat, asr])
        Issue.record("chat(text-out)→transcribe(audio-in) should be rejected")
    } catch let error as PipelineValidationError {
        #expect(
            error == .incompatibleEdge(from: 0, to: 1, produced: .text, expected: .audio))
    }
}

@Test func rejectsModelThatDoesNotServeCapability() throws {
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    do {
        _ = try PipelineValidator.validate([stage(chat, .speak)], shelf: [chat])
        Issue.record("a chat-only model can't be a speak stage")
    } catch let error as PipelineValidationError {
        #expect(error == .modelLacksCapability(index: 0, modelID: chat.id, capability: .speak))
    }
}

@Test func rejectsUnknownCapability() throws {
    let vlm = model("vlm", name: "llava", capabilities: [.see])
    do {
        _ = try PipelineValidator.validate([stage(vlm, .see)], shelf: [vlm])
        Issue.record("see is absent from the v1 signature table")
    } catch let error as PipelineValidationError {
        #expect(error == .unknownCapability(index: 0, capability: .see))
    }
}

@Test func rejectsMissingAndNotReadyModels() throws {
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    do {
        _ = try PipelineValidator.validate(
            [PipelineStage(modelID: "ghost", capability: .chat)], shelf: [chat])
        Issue.record("missing model should throw")
    } catch let error as PipelineValidationError {
        #expect(error == .modelMissing(index: 0, modelID: "ghost"))
    }

    let unresolved = model("u", name: "half", capabilities: [.chat], state: .unresolved)
    do {
        _ = try PipelineValidator.validate([stage(unresolved, .chat)], shelf: [unresolved])
        Issue.record("not-ready model should throw")
    } catch let error as PipelineValidationError {
        #expect(error == .notReady(index: 0, modelID: unresolved.id))
    }
}

@Test func rejectsEmptyPipeline() throws {
    #expect(throws: PipelineValidationError.self) {
        _ = try PipelineValidator.validate([], shelf: [])
    }
}

@Test func headTailSignatures() throws {
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    let tts = model("tts", name: "kokoro", capabilities: [.speak])
    let chatOnly = try PipelineValidator.validate([stage(chat, .chat)], shelf: [chat, tts])
    #expect(chatOnly == PipelineSignature(input: .text, output: .text))

    let chatSpeak = try PipelineValidator.validate(
        [stage(chat, .chat), stage(tts, .speak)], shelf: [chat, tts])
    #expect(chatSpeak == PipelineSignature(input: .text, output: .audio))
}

@Test func nextCapabilitiesFollowTheTailPort() {
    let chat = model("chat", name: "gemma", capabilities: [.chat])
    let afterChat = PipelineValidator.nextCapabilities(after: [stage(chat, .chat)])
    #expect(afterChat.contains(.speak))
    #expect(afterChat.contains(.image))
    #expect(afterChat.contains(.chat))
    #expect(!afterChat.contains(.transcribe))
}
