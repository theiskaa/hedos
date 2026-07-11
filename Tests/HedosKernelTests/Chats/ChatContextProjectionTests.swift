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

@Test func mergedSameRoleTurnsCarryExplicitBoundary() {
    let turns = [
        turn(0, .user, "hi"),
        turn(1, .assistant, "part one"),
        turn(2, .assistant, "part two"),
    ]
    let messages = ChatFlow.messages(from: turns)
    #expect(messages.last?.content == "part one" + ChatFlow.mergeBoundary + "part two")
    #expect(messages.last?.content.contains("---") == true)
}

@Test func interruptedTurnProjectsWithAnnotation() {
    let interrupted = ChatTurn(
        id: "t1", sessionID: "s", seq: 1, role: .assistant, content: "half an answer",
        contentHash: "h1", createdAt: Date(timeIntervalSince1970: 0),
        updatedAt: Date(timeIntervalSince1970: 0), interrupted: true)
    let annotated = ChatFlow.messages(from: [turn(0, .user, "q"), interrupted])
    #expect(annotated.last?.content.hasSuffix(ChatFlow.interruptedMarker) == true)

    let clean = ChatFlow.messages(from: [turn(0, .user, "q"), turn(1, .assistant, "full answer")])
    #expect(clean.last?.content == "full answer")
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
    #expect(messages[0].content == "hi" + ChatFlow.mergeBoundary + "after")
    #expect(!messages[0].content.contains("an image prompt"))
}

@Test func assistantTurnWithToolCallsProjectsEvenWhenContentIsEmpty() {
    let call = ToolCall(id: "call-1", name: "get_time", arguments: .object([:]))
    var calling = turn(1, .assistant, "")
    calling.toolCallsJSON = [call].turnToolCallsJSON
    var result = turn(2, .tool, "12:00")
    result.toolCallID = "call-1"
    result.toolName = "get_time"
    let turns = [
        turn(0, .user, "what time is it"),
        calling,
        result,
        turn(3, .assistant, "It is noon."),
    ]
    let messages = ChatFlow.messages(from: turns)
    #expect(messages.count == 4)
    #expect(messages[1].role == .assistant)
    #expect(messages[1].toolCalls == [call])
    #expect(messages[2].role == .tool)
    #expect(messages[2].toolCallID == "call-1")
    #expect(messages[2].toolName == "get_time")
    #expect(messages[3].content == "It is noon.")
}

@Test func mergeNeverCrossesToolBoundaries() {
    let call = ToolCall(id: "call-1", name: "list", arguments: .object([:]))
    var calling = turn(0, .assistant, "checking")
    calling.toolCallsJSON = [call].turnToolCallsJSON
    var first = turn(1, .tool, "a.txt")
    first.toolCallID = "call-1"
    first.toolName = "list"
    var second = turn(2, .tool, "b.txt")
    second.toolCallID = "call-2"
    second.toolName = "list"
    let turns = [calling, first, second, turn(3, .assistant, "done")]
    let messages = ChatFlow.messages(from: turns)
    #expect(messages.count == 4)
    #expect(messages[1].content == "a.txt")
    #expect(messages[2].content == "b.txt")
    #expect(messages[3].content == "done")
}
