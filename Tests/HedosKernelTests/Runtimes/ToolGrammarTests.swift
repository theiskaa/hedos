import Foundation
import Testing

@testable import HedosKernel

private func timeSpec() -> ToolSpec {
    ToolSpec(
        name: "get_time",
        description: "Reads the clock",
        parameters: .object([
            "type": .string("object"),
            "properties": .object([
                "zone": .object(["type": .string("string")]),
                "precise": .object(["type": .string("boolean")]),
            ]),
            "required": .array([.string("zone")]),
        ]))
}

@Test func grammarConstrainsNamesAndArgumentShapes() throws {
    let grammar = try ToolGrammar.grammar(for: [timeSpec()])
    #expect(grammar.contains("root ::= \"<tool_call>\""))
    #expect(grammar.contains("\\\"get_time\\\""))
    #expect(grammar.contains("call ::= call-0"))
    #expect(grammar.contains("::= boolean"))
    #expect(grammar.contains("::= string"))
    #expect(grammar.contains("\\\"zone\\\""))
    #expect(grammar.contains("(space \",\" space \"\\\"precise\\\"\""))
}

@Test func grammarSupportsEnumIntegerNumberAndArrays() throws {
    let spec = ToolSpec(
        name: "search",
        description: "",
        parameters: .object([
            "type": .string("object"),
            "properties": .object([
                "kind": .object(["enum": .array([.string("content"), .string("filename")])]),
                "limit": .object(["type": .string("integer")]),
                "weights": .object([
                    "type": .string("array"),
                    "items": .object(["type": .string("number")]),
                ]),
            ]),
            "required": .array([.string("kind"), .string("limit"), .string("weights")]),
        ]))
    let grammar = try ToolGrammar.grammar(for: [spec])
    #expect(grammar.contains("::= \"\\\"content\\\"\" | \"\\\"filename\\\"\""))
    #expect(grammar.contains("::= integer"))
    #expect(grammar.contains("-item ::= number"))
}

@Test func grammarRejectsUnsupportedConstructs() {
    let bad = ToolSpec(
        name: "weird",
        description: "",
        parameters: .object([
            "type": .string("object"),
            "properties": .object([
                "thing": .object(["oneOf": .array([]), "type": .string("blob")])
            ]),
            "required": .array([.string("thing")]),
        ]))
    #expect(throws: ToolGrammarError.self) {
        _ = try ToolGrammar.grammar(for: [bad])
    }
    #expect(throws: ToolGrammarError.noTools) {
        _ = try ToolGrammar.grammar(for: [])
    }
}

@Test func scannerSplitsProseFromCompletedCallsAcrossChunks() {
    var scanner = ToolCallScanner()
    var text = ""
    var call: ToolCall?
    for piece in ["Let me check. <tool", "_call>{\"name\": \"get_time\", ", "\"arguments\": {\"zone\": \"UTC\"}}</tool_call>"] {
        let scanned = scanner.feed(piece)
        text += scanned.text
        if let found = scanned.call { call = found }
    }
    #expect(text == "Let me check. ")
    #expect(call?.name == "get_time")
    #expect(call?.arguments == .object(["zone": .string("UTC")]))
}

@Test func scannerReleasesFalseTagPrefixes() {
    var scanner = ToolCallScanner()
    let first = scanner.feed("a < b and <tool")
    #expect(first.text == "a < b and ")
    let second = scanner.feed("box is not a call")
    #expect(second.text == "<toolbox is not a call")
    #expect(scanner.flush().isEmpty)
}

@Test func toolBlockLandsInTheSystemMessage() {
    let messages = [ChatMessage(role: .user, content: "what time is it")]
    let extended = LlamaEngine.messagesWithToolBlock(messages, tools: [timeSpec()])
    #expect(extended.count == 2)
    #expect(extended[0].role == .system)
    #expect(extended[0].content.contains("get_time"))
    #expect(extended[0].content.contains("<tool_call>"))

    let seeded = [
        ChatMessage(role: .system, content: "Be brief."),
        ChatMessage(role: .user, content: "time?"),
    ]
    let merged = LlamaEngine.messagesWithToolBlock(seeded, tools: [timeSpec()])
    #expect(merged.count == 2)
    #expect(merged[0].content.hasPrefix("Be brief."))
    #expect(merged[0].content.contains("get_time"))
}

@Test func allOptionalObjectsStillProduceValidCommaPlacement() throws {
    let spec = ToolSpec(
        name: "search",
        description: "",
        parameters: .object([
            "type": .string("object"),
            "properties": .object([
                "query": .object(["type": .string("string")]),
                "kind": .object(["type": .string("string")]),
            ]),
        ]))
    let grammar = try ToolGrammar.grammar(for: [spec])
    let objectRule = grammar.split(separator: "\n").first { $0.hasPrefix("args-0 ::=") }
    let rule = try #require(objectRule.map(String.init))
    #expect(rule.contains(" | "))
    #expect(rule.contains("(space \",\" space"))
    #expect(!rule.contains(")? (\""))
}

@Test func hostileToolNamesAreRejectedNotSpliced() {
    let hostile = ToolSpec(
        name: "x\" | \"",
        description: "",
        parameters: .object(["type": .string("object"), "properties": .object([:])]))
    #expect(throws: ToolGrammarError.self) {
        _ = try ToolGrammar.grammar(for: [hostile])
    }
}

@Test func scannerKeepsTextAfterTheCallAndSurfacesUnparseableBlocks() {
    var scanner = ToolCallScanner()
    let scanned = scanner.feed(
        "<tool_call>{\"name\": \"get_time\", \"arguments\": {}}</tool_call>Thanks!")
    #expect(scanned.call?.name == "get_time")
    let tail = scanner.flush()
    #expect((scanned.text + tail).contains("Thanks!"))

    var broken = ToolCallScanner()
    let bad = broken.feed("<tool_call>{not json}</tool_call>")
    #expect(bad.call == nil)
    #expect(bad.text.contains("{not json}"))
}
