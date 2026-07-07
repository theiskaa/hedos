import Foundation
import Testing

@testable import HedosKernel

@Test func pacedRevealDrainsAdaptivelyAndFinishes() {
    var reveal = PacedReveal(baseChars: 12, drainDivisor: 24)
    reveal.append(String(repeating: "a", count: 24))
    var advanced = reveal.tick()
    #expect(advanced)
    #expect(reveal.revealedCount == 12)
    #expect(reveal.backlog == 12)

    reveal.append(String(repeating: "b", count: 2400))
    advanced = reveal.tick()
    #expect(advanced)
    #expect(reveal.revealedCount == 12 + (2412 / 24))

    reveal.finish()
    #expect(reveal.backlog == 0)
    advanced = reveal.tick()
    #expect(!advanced)
    #expect(reveal.revealed.count == 2424)

    reveal.reset()
    #expect(reveal.target.isEmpty)
    advanced = reveal.tick()
    #expect(!advanced)
}

@Test func balancerClosesTrailingMarkersWithoutTouchingCompleteText() {
    #expect(MarkdownBalancer.balanced("plain text") == "plain text")
    #expect(MarkdownBalancer.balanced("a **bold** done") == "a **bold** done")
    #expect(MarkdownBalancer.balanced("a **bold start") == "a **bold start**")
    #expect(MarkdownBalancer.balanced("a *tilt") == "a *tilt*")
    #expect(MarkdownBalancer.balanced("mid `code") == "mid `code`")
    #expect(MarkdownBalancer.balanced("nested **bold *ital") == "nested **bold *ital***")
    #expect(MarkdownBalancer.balanced("some __strong") == "some __strong__")
}

@Test func balancerHandlesFencesListsAndCodeSpans() {
    #expect(
        MarkdownBalancer.balanced("before\n```swift\nlet x = 1")
            == "before\n```swift\nlet x = 1\n```")
    #expect(
        MarkdownBalancer.balanced("```\ncode\n```\nafter **hey")
            == "```\ncode\n```\nafter **hey**")
    #expect(MarkdownBalancer.balanced("- item one\n- item two") == "- item one\n- item two")
    #expect(MarkdownBalancer.balanced("* bullet not italic") == "* bullet not italic")
    #expect(MarkdownBalancer.balanced("a `*not emphasis*` done") == "a `*not emphasis*` done")
    #expect(MarkdownBalancer.balanced("**bold across\nlines still open") == "**bold across\nlines still open**")
}
