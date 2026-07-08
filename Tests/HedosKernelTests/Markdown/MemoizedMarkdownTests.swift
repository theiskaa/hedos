import Foundation
import Testing

@testable import HedosKernel

@Test func memoizedMarkdownReparsesOnlyWhenTextChanges() {
    var cache = MemoizedMarkdown()
    let first = cache.blocks(for: "# heading")
    let second = cache.blocks(for: "# heading")
    #expect(first == second)
    #expect(cache.parseCount == 1)

    let third = cache.blocks(for: "different text")
    #expect(third == [.paragraph("different text")])
    #expect(cache.parseCount == 2)

    let fourth = cache.blocks(for: "different text")
    #expect(fourth == third)
    #expect(cache.parseCount == 2)
}
