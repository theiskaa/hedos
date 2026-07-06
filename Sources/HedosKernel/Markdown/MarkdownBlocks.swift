import Foundation

public enum MarkdownBlock: Sendable, Hashable {
    case paragraph(String)
    case heading(level: Int, text: String)
    case code(language: String?, code: String, closed: Bool)
    case list(items: [String], ordered: Bool)
    case quote(String)
    case table(header: [String], rows: [[String]])
    case rule
}

public enum MarkdownBlocks {
    public static func parse(_ text: String) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var listItems: [String] = []
        var listOrdered = false
        var quoteLines: [String] = []
        var tableLines: [String] = []
        var codeLanguage: String?
        var codeLines: [String] = []
        var inCode = false

        func flushParagraph() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph = []
            }
        }

        func flushList() {
            if !listItems.isEmpty {
                blocks.append(.list(items: listItems, ordered: listOrdered))
                listItems = []
            }
        }

        func flushQuote() {
            if !quoteLines.isEmpty {
                blocks.append(.quote(quoteLines.joined(separator: "\n")))
                quoteLines = []
            }
        }

        func flushTable() {
            guard !tableLines.isEmpty else { return }
            let rows = tableLines.map(tableCells)
            if rows.count >= 2, isTableDivider(tableLines[1]) {
                blocks.append(
                    .table(header: rows[0], rows: Array(rows.dropFirst(2))))
            } else {
                blocks.append(.paragraph(tableLines.joined(separator: "\n")))
            }
            tableLines = []
        }

        func flushAll() {
            flushParagraph()
            flushList()
            flushQuote()
            flushTable()
        }

        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(line)
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if inCode {
                if trimmed.hasPrefix("```") {
                    blocks.append(
                        .code(
                            language: codeLanguage,
                            code: codeLines.joined(separator: "\n"),
                            closed: true))
                    codeLines = []
                    codeLanguage = nil
                    inCode = false
                } else {
                    codeLines.append(line)
                }
                continue
            }

            if trimmed.hasPrefix("```") {
                flushAll()
                let language = trimmed.dropFirst(3).trimmingCharacters(in: .whitespaces)
                codeLanguage = language.isEmpty ? nil : language
                inCode = true
                continue
            }

            if trimmed.isEmpty {
                flushAll()
                continue
            }

            if let heading = headingLine(trimmed) {
                flushAll()
                blocks.append(heading)
                continue
            }

            if isRule(trimmed) {
                flushAll()
                blocks.append(.rule)
                continue
            }

            if trimmed.hasPrefix(">") {
                flushParagraph()
                flushList()
                flushTable()
                quoteLines.append(
                    String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces))
                continue
            }

            if let item = unorderedItem(trimmed) {
                flushParagraph()
                flushQuote()
                flushTable()
                if !listItems.isEmpty && listOrdered { flushList() }
                listOrdered = false
                listItems.append(item)
                continue
            }

            if let item = orderedItem(trimmed) {
                flushParagraph()
                flushQuote()
                flushTable()
                if !listItems.isEmpty && !listOrdered { flushList() }
                listOrdered = true
                listItems.append(item)
                continue
            }

            if trimmed.hasPrefix("|") {
                flushParagraph()
                flushList()
                flushQuote()
                tableLines.append(trimmed)
                continue
            }

            flushList()
            flushQuote()
            flushTable()
            paragraph.append(line)
        }

        if inCode {
            blocks.append(
                .code(
                    language: codeLanguage,
                    code: codeLines.joined(separator: "\n"),
                    closed: false))
        }
        flushAll()
        return blocks
    }

    private static func headingLine(_ line: String) -> MarkdownBlock? {
        var level = 0
        for character in line {
            if character == "#" { level += 1 } else { break }
        }
        guard (1...6).contains(level) else { return nil }
        let rest = line.dropFirst(level)
        guard rest.first == " " || rest.isEmpty else { return nil }
        return .heading(level: level, text: rest.trimmingCharacters(in: .whitespaces))
    }

    private static func isRule(_ line: String) -> Bool {
        let stripped = line.replacingOccurrences(of: " ", with: "")
        guard stripped.count >= 3 else { return false }
        return stripped.allSatisfy { $0 == "-" } || stripped.allSatisfy { $0 == "*" }
            || stripped.allSatisfy { $0 == "_" }
    }

    private static func unorderedItem(_ line: String) -> String? {
        for marker in ["- ", "* ", "+ "] where line.hasPrefix(marker) {
            return String(line.dropFirst(marker.count)).trimmingCharacters(in: .whitespaces)
        }
        return nil
    }

    private static func orderedItem(_ line: String) -> String? {
        var digits = ""
        var rest = Substring(line)
        while let first = rest.first, first.isNumber {
            digits.append(first)
            rest = rest.dropFirst()
        }
        guard !digits.isEmpty, rest.hasPrefix(". ") || rest.hasPrefix(") ") else { return nil }
        return String(rest.dropFirst(2)).trimmingCharacters(in: .whitespaces)
    }

    private static func tableCells(_ line: String) -> [String] {
        var trimmed = line
        if trimmed.hasPrefix("|") { trimmed = String(trimmed.dropFirst()) }
        if trimmed.hasSuffix("|") { trimmed = String(trimmed.dropLast()) }
        return trimmed.split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    private static func isTableDivider(_ line: String) -> Bool {
        tableCells(line).allSatisfy { cell in
            !cell.isEmpty
                && cell.allSatisfy { $0 == "-" || $0 == ":" }
        }
    }
}
