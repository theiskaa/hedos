import Foundation
import Testing

@testable import HedosKernel

@Test func parsesBareKeysStringsBoolsAndArrays() throws {
    let table = try TOMLLite.parse(
        """
        id = "python:demo"
        network = false
        count = 3
        capabilities = ["chat", "complete"]
        empty = []
        """)
    #expect(table["id"]?.stringValue == "python:demo")
    #expect(table["network"]?.boolValue == false)
    #expect(table["count"] == .int(3))
    #expect(table["capabilities"]?.stringArray == ["chat", "complete"])
    #expect(table["empty"]?.stringArray == [])
}

@Test func parsesTableHeadersAndInlineTables() throws {
    let table = try TOMLLite.parse(
        """
        detect = { file = "model_index.json", contains = "FluxPipeline" }

        [env]
        manager = "uv"

        [permissions]
        network = true
        """)
    let detect = table["detect"]?.tableValue
    #expect(detect?["file"]?.stringValue == "model_index.json")
    #expect(detect?["contains"]?.stringValue == "FluxPipeline")
    #expect(table["env"]?.tableValue?["manager"]?.stringValue == "uv")
    #expect(table["permissions"]?.tableValue?["network"]?.boolValue == true)
}

@Test func ignoresFullLineAndTrailingComments() throws {
    let table = try TOMLLite.parse(
        """
        # full line comment
        id = "demo"  # trailing comment
        hash = "a#b"
        """)
    #expect(table["id"]?.stringValue == "demo")
    #expect(table["hash"]?.stringValue == "a#b")
}

@Test func parsesEscapesInStrings() throws {
    let table = try TOMLLite.parse(#"text = "line\nquote \"x\" slash \\""#)
    #expect(table["text"]?.stringValue == "line\nquote \"x\" slash \\")
}

@Test func malformedLineReportsLineNumber() {
    do {
        _ = try TOMLLite.parse("id = \"ok\"\nbroken line without equals")
        Issue.record("expected a parse error")
    } catch let error as TOMLParseError {
        #expect(error.line == 2)
    } catch {
        Issue.record("unexpected error type")
    }
}

@Test func unsupportedSyntaxIsAnErrorNotASilentSkip() {
    #expect(throws: TOMLParseError.self) { _ = try TOMLLite.parse("value = 3.14") }
    #expect(throws: TOMLParseError.self) { _ = try TOMLLite.parse("a.b = \"dotted\"") }
    #expect(throws: TOMLParseError.self) {
        _ = try TOMLLite.parse("text = \"\"\"multi\nline\"\"\"")
    }
    #expect(throws: TOMLParseError.self) { _ = try TOMLLite.parse("open = \"unterminated") }
}
