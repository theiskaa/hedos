import Foundation
import Testing

@testable import HedosKernel

@Test func inlinedToolTranscriptEscapesNamesWithSpecialCharacters() throws {
    let message = ChatMessage(
        role: .assistant, content: "",
        toolCalls: [
            ToolCall(name: "weird\"na\\me", arguments: .object(["p": .string("v")]))
        ])
    let text = message.inlinedToolTranscript.content
    let inner = try #require(
        text.range(of: "<tool_call>").flatMap { open in
            text.range(of: "</tool_call>").map { close in
                String(text[open.upperBound..<close.lowerBound])
            }
        })
    let parsed = try JSONSerialization.jsonObject(with: Data(inner.utf8)) as? [String: Any]
    #expect(parsed?["name"] as? String == "weird\"na\\me")
}

@Test func parseAllWrapsBarePromptAsUserMessage() throws {
    let messages = try ChatMessage.parseAll(from: ["prompt": .string("hello")])
    #expect(messages == [ChatMessage(role: .user, content: "hello")])
}

@Test func parseAllParsesAMessagesArray() throws {
    let messages = try ChatMessage.parseAll(from: [
        "messages": .array([
            .object(["role": .string("system"), "content": .string("be terse")]),
            .object(["role": .string("user"), "content": .string("hi")]),
        ])
    ])
    #expect(messages.map(\.role) == [.system, .user])
    #expect(messages.map(\.content) == ["be terse", "hi"])
}

@Test func parseAllThrowsOnNonStringContentNamingIndex() {
    let object: [String: JSONValue] = [
        "messages": .array([
            .object(["role": .string("user"), "content": .string("ok")]),
            .object(["role": .string("user"), "content": .int(5)]),
        ])
    ]
    do {
        _ = try ChatMessage.parseAll(from: object)
        Issue.record("expected a throw for non-string content")
    } catch let KernelError.payloadInvalid(message) {
        #expect(message.contains("index 1"))
        #expect(message.contains("non-string"))
    } catch {
        Issue.record("expected payloadInvalid, got \(error)")
    }
}

@Test func parseAllThrowsOnUnknownRole() {
    let object: [String: JSONValue] = [
        "messages": .array([
            .object(["role": .string("wizard"), "content": .string("hi")])
        ])
    ]
    #expect(throws: KernelError.self) {
        _ = try ChatMessage.parseAll(from: object)
    }
}

@Test func parseAllRequiresMessagesOrPrompt() {
    #expect(throws: KernelError.self) {
        _ = try ChatMessage.parseAll(from: [:])
    }
}
