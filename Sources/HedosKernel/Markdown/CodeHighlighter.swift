import Foundation

public enum CodeTokenKind: Sendable, Hashable {
    case plain
    case keyword
    case string
    case comment
    case number
}

public struct CodeToken: Sendable, Hashable {
    public let text: String
    public let kind: CodeTokenKind

    public init(text: String, kind: CodeTokenKind) {
        self.text = text
        self.kind = kind
    }
}

public enum CodeHighlighter {
    public static func tokens(_ code: String, language: String?) -> [CodeToken] {
        let keywords = Self.keywords(for: language)
        let lineComment = Self.lineComment(for: language)
        var tokens: [CodeToken] = []
        var plain = ""

        func flushPlain() {
            if !plain.isEmpty {
                tokens.append(CodeToken(text: plain, kind: .plain))
                plain = ""
            }
        }

        var rest = Substring(code)
        while let character = rest.first {
            if let lineComment, rest.hasPrefix(lineComment) {
                flushPlain()
                let comment = rest.prefix { $0 != "\n" }
                tokens.append(CodeToken(text: String(comment), kind: .comment))
                rest = rest.dropFirst(comment.count)
                continue
            }
            if character == "\"" || character == "'" || character == "`" {
                flushPlain()
                var literal = String(character)
                var scan = rest.dropFirst()
                while let next = scan.first, next != character, next != "\n" {
                    literal.append(next)
                    scan = scan.dropFirst()
                    if next == "\\", let escaped = scan.first {
                        literal.append(escaped)
                        scan = scan.dropFirst()
                    }
                }
                if let closing = scan.first, closing == character {
                    literal.append(closing)
                    scan = scan.dropFirst()
                }
                tokens.append(CodeToken(text: literal, kind: .string))
                rest = scan
                continue
            }
            if character.isNumber, plainEndsAtBoundary(plain) {
                flushPlain()
                let number = rest.prefix { $0.isNumber || $0 == "." || $0 == "_" || $0 == "x" }
                tokens.append(CodeToken(text: String(number), kind: .number))
                rest = rest.dropFirst(number.count)
                continue
            }
            if character.isLetter || character == "_" {
                flushPlain()
                let word = rest.prefix { $0.isLetter || $0.isNumber || $0 == "_" }
                tokens.append(
                    CodeToken(
                        text: String(word),
                        kind: keywords.contains(String(word)) ? .keyword : .plain))
                rest = rest.dropFirst(word.count)
                continue
            }
            plain.append(character)
            rest = rest.dropFirst()
        }
        flushPlain()
        return Self.mergedPlainRuns(tokens)
    }

    private static func plainEndsAtBoundary(_ plain: String) -> Bool {
        guard let last = plain.last else { return true }
        return !(last.isLetter || last.isNumber || last == "_")
    }

    private static func mergedPlainRuns(_ tokens: [CodeToken]) -> [CodeToken] {
        var merged: [CodeToken] = []
        for token in tokens {
            if token.kind == .plain, let last = merged.last, last.kind == .plain {
                merged[merged.count - 1] = CodeToken(text: last.text + token.text, kind: .plain)
            } else {
                merged.append(token)
            }
        }
        return merged
    }

    private static func lineComment(for language: String?) -> String? {
        switch language?.lowercased() {
        case "python", "ruby", "sh", "bash", "shell", "zsh", "yaml", "toml", "r":
            "#"
        case "lua", "sql":
            "--"
        case "html", "xml", "css", "json":
            nil
        default:
            "//"
        }
    }

    private static func keywords(for language: String?) -> Set<String> {
        let shared: Set<String> = [
            "if", "else", "for", "while", "return", "break", "continue", "switch", "case",
            "default", "true", "false", "nil", "null", "None", "True", "False", "in", "not",
            "and", "or", "import", "from", "as", "try", "catch", "except", "finally", "throw",
            "throws", "raise", "new", "delete", "this", "self", "super", "static", "public",
            "private", "protected", "internal", "final", "abstract", "interface", "protocol",
            "extension", "typealias", "type", "async", "await", "yield", "defer", "guard",
            "do", "then", "end", "begin", "match", "when", "where", "select",
        ]
        let declarations: Set<String> = [
            "func", "def", "fn", "function", "var", "let", "const", "val", "class", "struct",
            "enum", "actor", "trait", "impl", "mut", "pub", "use", "mod", "package", "module",
            "namespace", "void", "int", "float", "double", "bool", "string", "char", "elif",
            "lambda", "with", "pass", "global", "nonlocal", "assert", "is", "del", "print",
        ]
        _ = language
        return shared.union(declarations)
    }
}
