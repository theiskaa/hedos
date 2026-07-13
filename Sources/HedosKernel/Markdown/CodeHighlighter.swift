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
        let normalizedLanguage = language?.lowercased()
        let isShell = normalizedLanguage.map {
            ["sh", "bash", "zsh", "shell", "curl"].contains($0)
        } ?? false
        let isJSON = normalizedLanguage == "json"
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
                    if next == "\\", let escaped = scan.first, escaped != "\n" {
                        literal.append(escaped)
                        scan = scan.dropFirst()
                    }
                }
                if let closing = scan.first, closing == character {
                    literal.append(closing)
                    scan = scan.dropFirst()
                }
                var stringKind: CodeTokenKind = .string
                if isJSON, character == "\"" {
                    var lookahead = scan
                    while let next = lookahead.first, next == " " || next == "\t" {
                        lookahead = lookahead.dropFirst()
                    }
                    if lookahead.first == ":" {
                        stringKind = .keyword
                    }
                }
                tokens.append(CodeToken(text: literal, kind: stringKind))
                rest = scan
                continue
            }
            if isShell, character == "-", let after = rest.dropFirst().first,
                after.isLetter || after == "-"
            {
                flushPlain()
                let flag = rest.prefix { $0 == "-" || $0.isLetter || $0.isNumber }
                tokens.append(CodeToken(text: String(flag), kind: .keyword))
                rest = rest.dropFirst(flag.count)
                continue
            }
            if character.isNumber, plainEndsAtBoundary(plain) {
                flushPlain()
                let allowsHex = character == "0"
                let initialScan = rest.prefix {
                    $0.isNumber || $0 == "." || $0 == "_" || (allowsHex && ($0 == "x" || $0 == "X"))
                }
                var numberText = String(initialScan)
                var consumedCount = initialScan.count
                if numberText.count >= 2 {
                    let leadingTwo = numberText.prefix(2)
                    if leadingTwo == "0x" || leadingTwo == "0X" {
                        let remainder = rest.dropFirst(consumedCount)
                        let hexTail = remainder.prefix { $0.isHexDigit || $0 == "_" }
                        numberText += String(hexTail)
                        consumedCount += hexTail.count
                    }
                }
                tokens.append(CodeToken(text: numberText, kind: .number))
                rest = rest.dropFirst(consumedCount)
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
        case "python", "ruby", "sh", "bash", "shell", "zsh", "yaml", "toml", "r", "curl":
            "#"
        case "lua", "sql":
            "--"
        case "html", "xml", "css", "json":
            nil
        default:
            "//"
        }
    }

    private static let commonKeywords: Set<String> = [
        "if", "else", "for", "while", "return", "break", "continue", "switch", "case",
        "default", "true", "false", "nil", "null", "in", "not", "and", "or", "import",
        "from", "as", "try", "catch", "except", "finally", "throw", "throws", "new",
        "delete", "this", "self", "super", "static", "public", "private", "protected",
        "do", "then", "end", "match", "when", "where",
    ]

    private static func keywords(for language: String?) -> Set<String> {
        switch language?.lowercased() {
        case "swift":
            return commonKeywords.union([
                "func", "var", "let", "guard", "defer", "async", "await", "struct", "enum",
                "class", "extension", "protocol", "typealias", "internal", "final", "actor",
                "init", "associatedtype", "indirect", "mutating", "inout", "some", "any",
                "willSet", "didSet", "subscript", "fileprivate", "open", "lazy", "weak",
                "unowned", "rethrows", "operator", "precedencegroup",
            ])
        case "python", "py":
            return commonKeywords.union([
                "def", "elif", "lambda", "with", "pass", "global", "nonlocal", "assert",
                "is", "del", "print", "None", "True", "False", "class", "yield", "raise",
                "async", "await",
            ])
        case "javascript", "js", "typescript", "ts", "jsx", "tsx":
            return commonKeywords.union([
                "function", "var", "let", "const", "class", "extends", "implements",
                "interface", "typeof", "instanceof", "void", "yield", "async", "await",
                "export", "default", "undefined", "of", "get", "set", "type", "enum",
                "namespace", "declare", "readonly", "abstract",
            ])
        case "java":
            return commonKeywords.union([
                "class", "interface", "extends", "implements", "void", "int", "float",
                "double", "boolean", "char", "long", "short", "byte", "package", "final",
                "abstract", "synchronized", "volatile", "transient", "native", "instanceof",
                "enum",
            ])
        case "c", "cpp", "c++", "objc", "objective-c":
            return commonKeywords.union([
                "int", "float", "double", "char", "void", "struct", "union", "enum",
                "typedef", "const", "extern", "sizeof", "unsigned", "signed", "long",
                "short", "class", "namespace", "template", "virtual", "override",
                "nullptr",
            ])
        case "go", "golang":
            return commonKeywords.union([
                "func", "var", "const", "type", "struct", "interface", "package", "go",
                "chan", "select", "defer", "map", "range", "fallthrough",
            ])
        case "rust", "rs":
            return commonKeywords.union([
                "fn", "let", "mut", "const", "struct", "enum", "trait", "impl", "pub",
                "use", "mod", "loop", "move", "ref", "unsafe", "async", "await", "dyn",
                "crate", "extern",
            ])
        case "ruby", "rb":
            return commonKeywords.union([
                "def", "end", "class", "module", "require", "require_relative",
                "attr_accessor", "yield", "begin", "rescue", "ensure", "raise", "unless",
                "until", "elsif", "puts",
            ])
        case "kotlin", "kt":
            return commonKeywords.union([
                "fun", "val", "var", "class", "object", "interface", "is", "as",
                "package", "companion", "init", "override", "open", "sealed", "data",
                "suspend", "inline", "internal",
            ])
        case "csharp", "cs", "c#":
            return commonKeywords.union([
                "class", "interface", "namespace", "using", "void", "int", "float",
                "double", "bool", "string", "override", "virtual", "abstract", "var",
                "readonly", "sealed",
            ])
        case "php":
            return commonKeywords.union([
                "function", "echo", "class", "namespace", "use", "require",
                "require_once", "include", "include_once", "array", "foreach",
                "elseif", "endif", "var",
            ])
        case "sh", "bash", "zsh", "shell", "curl":
            return commonKeywords.union([
                "fi", "done", "function", "echo", "esac", "local", "export",
                "curl", "cd", "cat", "ls", "grep", "sed", "awk", "set", "unset",
                "source", "exit", "read", "wget", "sudo", "chmod", "mkdir", "rm",
            ])
        case "json":
            return ["true", "false", "null"]
        case "sql":
            return commonKeywords.union([
                "select", "insert", "update", "delete", "join", "on", "group", "order",
                "by", "having", "table", "create", "drop", "alter", "values", "into",
            ])
        case "lua":
            return commonKeywords.union([
                "function", "local", "repeat", "until",
            ])
        default:
            return commonKeywords
        }
    }
}
