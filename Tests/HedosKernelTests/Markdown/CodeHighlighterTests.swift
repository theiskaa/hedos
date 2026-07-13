import Foundation
import Testing

@testable import HedosKernel

@Test func tokensRecognizeKeywordsInSwiftAndLeaveRestPlain() {
    let tokens = CodeHighlighter.tokens("let x = 1", language: "swift")
    #expect(
        tokens == [
            CodeToken(text: "let", kind: .keyword),
            CodeToken(text: " x = ", kind: .plain),
            CodeToken(text: "1", kind: .number),
        ])
}

@Test func tokensRecognizeKeywordsAreLanguageAware() {
    let swiftTokens = CodeHighlighter.tokens("def", language: "swift")
    let pythonTokens = CodeHighlighter.tokens("def", language: "python")
    let unknownTokens = CodeHighlighter.tokens("def", language: "not-a-real-language")
    #expect(swiftTokens == [CodeToken(text: "def", kind: .plain)])
    #expect(pythonTokens == [CodeToken(text: "def", kind: .keyword)])
    #expect(unknownTokens == [CodeToken(text: "def", kind: .plain)])
}

@Test func tokensRecognizeSwiftSpecificKeywordsNotSharedAcrossLanguages() {
    let tokens = CodeHighlighter.tokens("func guard", language: "swift")
    #expect(
        tokens == [
            CodeToken(text: "func", kind: .keyword),
            CodeToken(text: " ", kind: .plain),
            CodeToken(text: "guard", kind: .keyword),
        ])
    let pythonTokens = CodeHighlighter.tokens("func", language: "python")
    #expect(pythonTokens == [CodeToken(text: "func", kind: .plain)])
}

@Test func tokensRecognizeStringLiterals() {
    let tokens = CodeHighlighter.tokens("\"hello world\"", language: "swift")
    #expect(tokens == [CodeToken(text: "\"hello world\"", kind: .string)])
}

@Test func tokensKeepKeywordInsideStringLiteralAsString() {
    let tokens = CodeHighlighter.tokens("\"return true\"", language: "swift")
    #expect(tokens == [CodeToken(text: "\"return true\"", kind: .string)])
}

@Test func tokensHandleUnterminatedStringLiteral() {
    let tokens = CodeHighlighter.tokens("\"unterminated", language: "swift")
    #expect(tokens == [CodeToken(text: "\"unterminated", kind: .string)])
}

@Test func tokensHandleEscapedQuoteInsideStringLiteral() {
    let tokens = CodeHighlighter.tokens("\"a \\\" b\"", language: "swift")
    #expect(tokens == [CodeToken(text: "\"a \\\" b\"", kind: .string)])
}

@Test func tokensRecognizeSingleAndBacktickQuotedLiterals() {
    let single = CodeHighlighter.tokens("'a'", language: "swift")
    #expect(single == [CodeToken(text: "'a'", kind: .string)])

    let backtick = CodeHighlighter.tokens("`x`", language: "swift")
    #expect(backtick == [CodeToken(text: "`x`", kind: .string)])
}

@Test func tokensRecognizeLineCommentForDefaultLanguage() {
    let tokens = CodeHighlighter.tokens("// a comment", language: "swift")
    #expect(tokens == [CodeToken(text: "// a comment", kind: .comment)])
}

@Test func tokensKeepKeywordInsideCommentAsComment() {
    let tokens = CodeHighlighter.tokens("// return true", language: "swift")
    #expect(tokens == [CodeToken(text: "// return true", kind: .comment)])
}

@Test func tokensRecognizeHashLineCommentForPython() {
    let tokens = CodeHighlighter.tokens("# a comment", language: "python")
    #expect(tokens == [CodeToken(text: "# a comment", kind: .comment)])
}

@Test func tokensRecognizeDoubleDashLineCommentForLua() {
    let tokens = CodeHighlighter.tokens("-- a comment", language: "lua")
    #expect(tokens == [CodeToken(text: "-- a comment", kind: .comment)])
}

@Test func tokensDoNotRecognizeLineCommentsForCSSLikeLanguages() {
    let tokens = CodeHighlighter.tokens("// not a comment", language: "css")
    #expect(tokens.contains(where: { $0.kind == .comment }) == false)
}

@Test func tokensRecognizeNumbers() {
    let tokens = CodeHighlighter.tokens("42", language: "swift")
    #expect(tokens == [CodeToken(text: "42", kind: .number)])
}

@Test func tokensRecognizeFloatingPointAndFullHexLiteral() {
    let tokens = CodeHighlighter.tokens("3.14 0x1F", language: "swift")
    #expect(
        tokens == [
            CodeToken(text: "3.14", kind: .number),
            CodeToken(text: " ", kind: .plain),
            CodeToken(text: "0x1F", kind: .number),
        ])
}

@Test func tokensRecognizeHexLiteralWithMixedCaseDigitsAsOneToken() {
    let tokens = CodeHighlighter.tokens("0xDEAD_beef", language: "swift")
    #expect(tokens == [CodeToken(text: "0xDEAD_beef", kind: .number)])
}

@Test func tokensDoNotAbsorbTrailingLetterIntoANonHexNumber() {
    let tokens = CodeHighlighter.tokens("3x", language: "swift")
    #expect(tokens.first == CodeToken(text: "3", kind: .number))
}

@Test func tokensStopStringLiteralAtABackslashBeforeNewline() {
    let tokens = CodeHighlighter.tokens("\"a\\\nb\"", language: "swift")
    #expect(tokens.first == CodeToken(text: "\"a\\", kind: .string))
}

@Test func tokensDoNotTreatDigitsInsideIdentifiersAsNumbers() {
    let tokens = CodeHighlighter.tokens("var1", language: "swift")
    #expect(tokens == [CodeToken(text: "var1", kind: .plain)])
}

@Test func tokensForNilLanguageUseDefaultSlashSlashComment() {
    let tokens = CodeHighlighter.tokens("// hi", language: nil)
    #expect(tokens == [CodeToken(text: "// hi", kind: .comment)])
}

@Test func tokensForUnrecognizedLanguageStringUseDefaultSlashSlashComment() {
    let tokens = CodeHighlighter.tokens("// hi", language: "not-a-real-language")
    #expect(tokens == [CodeToken(text: "// hi", kind: .comment)])
}

@Test func tokensForNilLanguageStillRecognizeSharedKeywords() {
    let tokens = CodeHighlighter.tokens("if true", language: nil)
    #expect(tokens.contains(CodeToken(text: "if", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "true", kind: .keyword)))
}

@Test func tokensRoundTripInvariantHoldsAcrossVariousInputs() {
    let samples: [(String, String?)] = [
        ("let x = 1", "swift"),
        ("def foo():\n    return True", "python"),
        ("-- comment\nlocal x = 1", "lua"),
        ("\"string with \\\" escape\" and 3.14 0x1F", "swift"),
        ("", "swift"),
        ("plain text with no tokens at all", nil),
        ("mixed 'quotes' and \"double\" and `backtick`", "javascript"),
        ("// comment\nvar1 = 42", "javascript"),
    ]
    for (code, language) in samples {
        let tokens = CodeHighlighter.tokens(code, language: language)
        let reconstructed = tokens.map(\.text).joined()
        #expect(reconstructed == code)
    }
}

@Test func tokensForEmptyStringReturnEmptyArray() {
    #expect(CodeHighlighter.tokens("", language: "swift") == [])
}

@Test func tokensHighlightJSONKeysDistinctlyFromStringValues() {
    let tokens = CodeHighlighter.tokens(
        "{\"model\": \"gpt\", \"stream\": true, \"n\": 42}", language: "json")
    #expect(tokens.contains(CodeToken(text: "\"model\"", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "\"gpt\"", kind: .string)))
    #expect(tokens.contains(CodeToken(text: "\"stream\"", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "true", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "42", kind: .number)))
    #expect(tokens.map(\.text).joined() == "{\"model\": \"gpt\", \"stream\": true, \"n\": 42}")
}

@Test func tokensHighlightJSONBooleanAndNullLiterals() {
    let tokens = CodeHighlighter.tokens("[true, false, null]", language: "json")
    #expect(tokens.contains(CodeToken(text: "true", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "false", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "null", kind: .keyword)))
}

@Test func tokensJSONHasNoLineComment() {
    let tokens = CodeHighlighter.tokens("// x", language: "json")
    #expect(tokens.contains { $0.kind == .comment } == false)
}

@Test func tokensHighlightShellCurlKeywordAndFlags() {
    let tokens = CodeHighlighter.tokens("curl -s --data-raw", language: "bash")
    #expect(tokens.contains(CodeToken(text: "curl", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "-s", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "--data-raw", kind: .keyword)))
    #expect(tokens.map(\.text).joined() == "curl -s --data-raw")
}

@Test func tokensRouteCurlLanguageToShellSet() {
    let tokens = CodeHighlighter.tokens("curl -H", language: "curl")
    #expect(tokens.contains(CodeToken(text: "curl", kind: .keyword)))
    #expect(tokens.contains(CodeToken(text: "-H", kind: .keyword)))
}

@Test func tokensDoNotTreatDashAsFlagOutsideShell() {
    let tokens = CodeHighlighter.tokens("-H", language: "json")
    #expect(tokens.contains { $0.kind == .keyword } == false)
}

@Test func tokensCoverPythonSDKKeywords() {
    let keywords = [
        "def", "import", "from", "class", "return", "None", "True", "False",
        "for", "in", "with", "as", "print",
    ]
    for keyword in keywords {
        let tokens = CodeHighlighter.tokens(keyword, language: "python")
        #expect(tokens == [CodeToken(text: keyword, kind: .keyword)])
    }
}
