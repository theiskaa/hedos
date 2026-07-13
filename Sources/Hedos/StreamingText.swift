import Foundation

struct PacedReveal: Sendable {
    private(set) var target = ""
    private(set) var revealedCount = 0
    private var targetCount = 0
    private let baseChars: Int
    private let drainDivisor: Int

    init(baseChars: Int = 12, drainDivisor: Int = 24) {
        self.baseChars = baseChars
        self.drainDivisor = drainDivisor
    }

    var backlog: Int {
        max(0, targetCount - revealedCount)
    }

    var revealed: String {
        revealedCount >= targetCount ? target : String(target.prefix(revealedCount))
    }

    mutating func append(_ delta: String) {
        guard !delta.isEmpty else { return }
        if target.isEmpty {
            target = delta
            targetCount = delta.count
        } else {
            let boundary = String(target.suffix(1))
            target += delta
            targetCount += (boundary + delta).count - 1
        }
    }

    mutating func tick() -> Bool {
        let pending = backlog
        guard pending > 0 else { return false }
        let step = max(baseChars, pending / drainDivisor)
        revealedCount = min(targetCount, revealedCount + step)
        return true
    }

    mutating func finish() {
        revealedCount = targetCount
    }

    mutating func reset() {
        target = ""
        revealedCount = 0
        targetCount = 0
    }
}

enum MarkdownBalancer {
    static func balanced(_ partial: String) -> String {
        var inFence = false
        var stack: [String] = []
        var inInlineCode = false

        for rawLine in partial.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                inFence.toggle()
                inInlineCode = false
                stack = []
                continue
            }
            if inFence { continue }
            inInlineCode = false
            scanInline(line, stack: &stack, inInlineCode: &inInlineCode)
        }

        if inFence {
            let needsNewline = !partial.hasSuffix("\n")
            return partial + (needsNewline ? "\n```" : "```")
        }
        var closed = partial
        if inInlineCode {
            closed += "`"
        }
        for marker in stack.reversed() {
            closed += marker
        }
        return closed
    }

    private static func scanInline(
        _ line: String, stack: inout [String], inInlineCode: inout Bool
    ) {
        var index = line.startIndex
        while index < line.endIndex {
            let character = line[index]
            if character == "`" {
                inInlineCode.toggle()
                index = line.index(after: index)
                continue
            }
            if inInlineCode {
                index = line.index(after: index)
                continue
            }
            if character == "*" || character == "_" {
                let doubled =
                    line.index(after: index) < line.endIndex
                    && line[line.index(after: index)] == character
                let marker = doubled ? String(repeating: String(character), count: 2)
                    : String(character)
                if !doubled && isListBullet(line, at: index) {
                    index = line.index(after: index)
                    continue
                }
                if stack.last == marker {
                    stack.removeLast()
                } else {
                    stack.append(marker)
                }
                index = doubled ? line.index(index, offsetBy: 2) : line.index(after: index)
                continue
            }
            index = line.index(after: index)
        }
    }

    private static func isListBullet(_ line: String, at index: String.Index) -> Bool {
        let before = line[..<index]
        guard before.allSatisfy(\.isWhitespace) else { return false }
        let after = line.index(after: index)
        return after == line.endIndex || line[after] == " "
    }
}
