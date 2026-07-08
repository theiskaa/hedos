import Foundation
import Testing

@testable import HedosKernel

@Test func parsePlainParagraphAndMultiLineJoin() {
    let single = MarkdownBlocks.parse("just one line")
    #expect(single == [.paragraph("just one line")])

    let multi = MarkdownBlocks.parse("line one\nline two\nline three")
    #expect(multi == [.paragraph("line one\nline two\nline three")])
}

@Test func parseHeadingLevelsOneThroughSix() {
    for level in 1...6 {
        let hashes = String(repeating: "#", count: level)
        let blocks = MarkdownBlocks.parse("\(hashes) Title \(level)")
        #expect(blocks == [.heading(level: level, text: "Title \(level)")])
    }
}

@Test func parseSevenHashesIsNotAHeading() {
    let blocks = MarkdownBlocks.parse("####### Title")
    #expect(blocks == [.paragraph("####### Title")])
}

@Test func parseHeadingWithoutSpaceAfterHashesIsNotAHeading() {
    let blocks = MarkdownBlocks.parse("#NoSpace")
    #expect(blocks == [.paragraph("#NoSpace")])
}

@Test func parseHeadingWithNoTrailingTextIsStillAHeading() {
    let blocks = MarkdownBlocks.parse("###")
    #expect(blocks == [.heading(level: 3, text: "")])
}

@Test func parseClosedCodeFenceWithLanguage() {
    let blocks = MarkdownBlocks.parse("```swift\nlet x = 1\nprint(x)\n```")
    #expect(blocks == [.code(language: "swift", code: "let x = 1\nprint(x)", closed: true)])
}

@Test func parseUnterminatedFenceCapturesPartialCodeAsOpen() {
    let blocks = MarkdownBlocks.parse("```swift\nlet x = 1\nprint(x")
    #expect(blocks == [.code(language: "swift", code: "let x = 1\nprint(x", closed: false)])
}

@Test func parseFenceWithNoLanguageTagIsNilLanguage() {
    let blocks = MarkdownBlocks.parse("```\nsome code\n```")
    #expect(blocks == [.code(language: nil, code: "some code", closed: true)])
}

@Test func parseAnyTripleBacktickLineClosesFenceRegardlessOfTrailingContent() {
    let blocks = MarkdownBlocks.parse("```swift\nlet x = 1\n```python\nafter")
    #expect(blocks == [
        .code(language: "swift", code: "let x = 1", closed: true),
        .paragraph("after"),
    ])
}

@Test func parseUnorderedList() {
    let blocks = MarkdownBlocks.parse("- one\n- two\n- three")
    #expect(blocks == [.list(items: ["one", "two", "three"], ordered: false)])
}

@Test func parseOrderedList() {
    let blocks = MarkdownBlocks.parse("1. one\n2. two\n3. three")
    #expect(blocks == [.list(items: ["one", "two", "three"], ordered: true)])
}

@Test func parseOrderedListAcceptsParenSeparator() {
    let blocks = MarkdownBlocks.parse("1) one\n2) two")
    #expect(blocks == [.list(items: ["one", "two"], ordered: true)])
}

@Test func parseListInterruptedByParagraphFlushesCorrectly() {
    let blocks = MarkdownBlocks.parse("- one\n- two\n\nafter the list")
    #expect(blocks == [
        .list(items: ["one", "two"], ordered: false),
        .paragraph("after the list"),
    ])
}

@Test func parseListInterruptedByNonBlankParagraphLineAlsoFlushes() {
    let blocks = MarkdownBlocks.parse("- one\n- two\nnot a list item")
    #expect(blocks == [
        .list(items: ["one", "two"], ordered: false),
        .paragraph("not a list item"),
    ])
}

@Test func parseSwitchingListOrderednessFlushesPreviousList() {
    let blocks = MarkdownBlocks.parse("- one\n- two\n1. three\n2. four")
    #expect(blocks == [
        .list(items: ["one", "two"], ordered: false),
        .list(items: ["three", "four"], ordered: true),
    ])
}

@Test func parseSingleLineQuote() {
    let blocks = MarkdownBlocks.parse("> a wise quote")
    #expect(blocks == [.quote("a wise quote")])
}

@Test func parseMultiLineQuoteJoinsLines() {
    let blocks = MarkdownBlocks.parse("> line one\n> line two")
    #expect(blocks == [.quote("line one\nline two")])
}

@Test func parseTableWithHeaderDividerAndRows() {
    let text = "| A | B |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |"
    let blocks = MarkdownBlocks.parse(text)
    #expect(blocks == [
        .table(header: ["A", "B"], rows: [["1", "2"], ["3", "4"]])
    ])
}

@Test func parseTableDividerAcceptsColonAlignmentMarkers() {
    let text = "| A | B |\n| :- | -: |\n| 1 | 2 |"
    let blocks = MarkdownBlocks.parse(text)
    #expect(blocks == [
        .table(header: ["A", "B"], rows: [["1", "2"]])
    ])
}

@Test func parseTwoLinePipeBlockWithoutDividerFallsBackToParagraph() {
    let text = "| A | B |\n| 1 | 2 |"
    let blocks = MarkdownBlocks.parse(text)
    #expect(blocks == [.paragraph("| A | B |\n| 1 | 2 |")])
}

@Test func parseSingleLinePipeBlockFallsBackToParagraph() {
    let blocks = MarkdownBlocks.parse("| just one row |")
    #expect(blocks == [.paragraph("| just one row |")])
}

@Test func parseHorizontalRuleVariants() {
    #expect(MarkdownBlocks.parse("---") == [.rule])
    #expect(MarkdownBlocks.parse("***") == [.rule])
    #expect(MarkdownBlocks.parse("___") == [.rule])
    #expect(MarkdownBlocks.parse("- - -") == [.rule])
}

@Test func parseMixedDocumentProducesFullSequenceOfBlocks() {
    let text = """
        # Title

        A paragraph of text.

        ```swift
        let x = 1
        ```

        - item one
        - item two
        """
    let blocks = MarkdownBlocks.parse(text)
    #expect(blocks == [
        .heading(level: 1, text: "Title"),
        .paragraph("A paragraph of text."),
        .code(language: "swift", code: "let x = 1", closed: true),
        .list(items: ["item one", "item two"], ordered: false),
    ])
}

@Test func parseEmptyStringReturnsEmptyArray() {
    #expect(MarkdownBlocks.parse("") == [])
}

@Test func parseWhitespaceOnlyStringReturnsEmptyArray() {
    #expect(MarkdownBlocks.parse("   \n\n   ") == [])
}
