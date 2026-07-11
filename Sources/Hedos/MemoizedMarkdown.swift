import Foundation
import HedosKernel

struct MemoizedMarkdown: Sendable {
    private var cachedText: String?
    private var cachedBlocks: [MarkdownBlock] = []
    private(set) var parseCount = 0

    init() {}

    mutating func blocks(for text: String) -> [MarkdownBlock] {
        if let cachedText, cachedText == text {
            return cachedBlocks
        }
        let blocks = MarkdownBlocks.parse(text)
        cachedText = text
        cachedBlocks = blocks
        parseCount += 1
        return blocks
    }
}
