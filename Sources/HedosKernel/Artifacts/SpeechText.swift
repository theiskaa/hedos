import Foundation

public enum SpeechText {
    public static func speakable(_ markdown: String) -> String {
        var lines: [String] = []
        var insideFence = false
        for rawLine in markdown.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(rawLine)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") || trimmed.hasPrefix("~~~") {
                insideFence.toggle()
                continue
            }
            if insideFence { continue }
            if trimmed.hasPrefix("|") && trimmed.hasSuffix("|") { continue }
            lines.append(stripInline(line))
        }
        return lines.joined(separator: "\n")
            .replacingOccurrences(
                of: "\n{3,}", with: "\n\n", options: .regularExpression)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func stripInline(_ line: String) -> String {
        var text = line
        text = text.replacingOccurrences(
            of: #"^\s{0,3}#{1,6}\s+"#, with: "", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"^\s{0,3}>\s?"#, with: "", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"^(\s*)[-*+]\s+"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"!?\[([^\]]*)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"\*{1,3}([^*]+)\*{1,3}"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"_{1,3}([^_]+)_{1,3}"#, with: "$1", options: .regularExpression)
        text = text.replacingOccurrences(
            of: #"`([^`]*)`"#, with: "$1", options: .regularExpression)
        return text
    }
}
