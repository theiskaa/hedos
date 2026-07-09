import Foundation
import Testing

@testable import HedosKernel

private func turn(
    _ seq: Int, _ role: TurnRole, _ content: String, artifactRefs: [String] = [],
    supersededBy: String? = nil
) -> ChatTurn {
    ChatTurn(
        id: "t\(seq)",
        sessionID: "s",
        seq: seq,
        role: role,
        content: content,
        artifactRefs: artifactRefs,
        supersededBy: supersededBy,
        contentHash: "h\(seq)",
        createdAt: Date(timeIntervalSince1970: 0),
        updatedAt: Date(timeIntervalSince1970: 0))
}

private func roles(_ messages: [ChatMessage]) -> [String] {
    messages.map(\.role.rawValue)
}

@Test func generatedImageTurnsAreOmittedFromChatContext() {
    let turns = [
        turn(0, .user, "hi"),
        turn(1, .assistant, "hello"),
        turn(2, .user, "a koala riding a bicycle"),
        turn(3, .assistant, "", artifactRefs: ["img_1"]),
        turn(4, .user, "what were we saying?"),
    ]

    let messages = ChatFlow.messages(from: turns)
    #expect(roles(messages) == ["user", "assistant", "user"])
    #expect(messages.map(\.content) == ["hi", "hello", "what were we saying?"])
}

@Test func generatedSpeechTurnsAreOmittedFromChatContext() {
    let turns = [
        turn(0, .user, "say something"),
        turn(1, .assistant, "", artifactRefs: ["wav_1"]),
        turn(2, .user, "now chat with me"),
    ]

    let messages = ChatFlow.messages(from: turns)
    #expect(roles(messages) == ["user"])
    #expect(messages.map(\.content) == ["now chat with me"])
}

@Test func chatContextNeverEmitsConsecutiveUserMessages() {
    let turns = [
        turn(0, .user, "hi"),
        turn(1, .assistant, "hello"),
        turn(2, .user, "an image prompt"),
        turn(3, .assistant, "", artifactRefs: ["img_1"]),
        turn(4, .user, "speak this"),
        turn(5, .assistant, "", artifactRefs: ["wav_1"]),
        turn(6, .user, "back to chat"),
    ]

    let messages = ChatFlow.messages(from: turns)
    for (previous, next) in zip(messages, messages.dropFirst()) {
        #expect(previous.role != next.role)
    }
}

@Test func narratedAssistantTurnsStayInChatContext() {
    let turns = [
        turn(0, .user, "hi"),
        turn(1, .assistant, "hello there", artifactRefs: ["wav_1"]),
        turn(2, .user, "more"),
    ]

    let messages = ChatFlow.messages(from: turns)
    #expect(roles(messages) == ["user", "assistant", "user"])
    #expect(messages[1].content == "hello there")
}

@Test func supersededTurnsDoNotBreakGenerationPairingOrAlternation() {
    let turns = [
        turn(0, .user, "hi"),
        turn(1, .assistant, "stale reply", supersededBy: "t9"),
        turn(2, .user, "an image prompt"),
        turn(3, .assistant, "", artifactRefs: ["img_1"]),
        turn(4, .user, "after"),
    ]

    let messages = ChatFlow.messages(from: turns)
    #expect(roles(messages) == ["user"])
    #expect(messages[0].content == "hi\n\nafter")
    #expect(!messages[0].content.contains("an image prompt"))
}
