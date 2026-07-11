import Foundation

public enum Diff {
    public static func unified(from before: String, to after: String, path: String) -> String {
        if before == after {
            return "\(path): no change"
        }
        let beforeLines = splitLines(before)
        let afterLines = splitLines(after)
        let hunks = diffLines(beforeLines, afterLines)

        var out = ["--- \(path)", "+++ \(path)"]
        out.append(contentsOf: hunks)
        if !hunks.contains(where: { $0.hasPrefix("+") || $0.hasPrefix("-") }) {
            let beforeNewline = before.hasSuffix("\n")
            let afterNewline = after.hasSuffix("\n")
            if beforeNewline != afterNewline {
                out.append(afterNewline ? "(final newline added)" : "(final newline removed)")
            }
        }
        return out.joined(separator: "\n")
    }

    static func splitLines(_ text: String) -> [String] {
        if text.isEmpty { return [] }
        var lines = text.components(separatedBy: "\n")
        if lines.last == "" {
            lines.removeLast()
        }
        return lines
    }

    static func diffLines(_ before: [String], _ after: [String]) -> [String] {
        let table = lcsTable(before, after)
        var operations: [String] = []
        var i = 0
        var j = 0
        while i < before.count && j < after.count {
            if before[i] == after[j] {
                operations.append(" " + before[i])
                i += 1
                j += 1
            } else if table[i + 1][j] >= table[i][j + 1] {
                operations.append("-" + before[i])
                i += 1
            } else {
                operations.append("+" + after[j])
                j += 1
            }
        }
        while i < before.count {
            operations.append("-" + before[i])
            i += 1
        }
        while j < after.count {
            operations.append("+" + after[j])
            j += 1
        }
        return operations
    }

    private static func lcsTable(_ before: [String], _ after: [String]) -> [[Int]] {
        var table = Array(
            repeating: Array(repeating: 0, count: after.count + 1), count: before.count + 1)
        if before.isEmpty || after.isEmpty { return table }
        for i in stride(from: before.count - 1, through: 0, by: -1) {
            for j in stride(from: after.count - 1, through: 0, by: -1) {
                if before[i] == after[j] {
                    table[i][j] = table[i + 1][j + 1] + 1
                } else {
                    table[i][j] = max(table[i + 1][j], table[i][j + 1])
                }
            }
        }
        return table
    }
}
