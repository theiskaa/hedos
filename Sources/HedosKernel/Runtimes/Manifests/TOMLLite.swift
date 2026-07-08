import Foundation

public enum TOMLValue: Sendable, Hashable {
    case string(String)
    case bool(Bool)
    case int(Int)
    case array([TOMLValue])
    case table([String: TOMLValue])

    public var stringValue: String? {
        if case .string(let value) = self { return value }
        return nil
    }

    public var boolValue: Bool? {
        if case .bool(let value) = self { return value }
        return nil
    }

    public var stringArray: [String]? {
        guard case .array(let values) = self else { return nil }
        return values.compactMap(\.stringValue)
    }

    public var tableValue: [String: TOMLValue]? {
        if case .table(let value) = self { return value }
        return nil
    }
}

public typealias TOMLTable = [String: TOMLValue]

public struct TOMLParseError: Error, Sendable, CustomStringConvertible {
    public let line: Int
    public let message: String

    public var description: String { "line \(line): \(message)" }
}

public enum TOMLLite {
    public static func parse(_ text: String) throws -> TOMLTable {
        var root: TOMLTable = [:]
        var currentTable: String?

        let rawLines = text.components(separatedBy: "\n")
        var index = 0
        while index < rawLines.count {
            let lineNumber = index + 1
            var stripped = try stripComment(rawLines[index], line: lineNumber)
                .trimmingCharacters(in: .whitespaces)
            index += 1
            if stripped.isEmpty { continue }

            if bracketBalance(stripped) > 0 {
                var balance = bracketBalance(stripped)
                while balance > 0, index < rawLines.count {
                    let continuation = try stripComment(rawLines[index], line: index + 1)
                        .trimmingCharacters(in: .whitespaces)
                    index += 1
                    if !continuation.isEmpty {
                        stripped += " " + continuation
                        balance += bracketBalance(continuation)
                    }
                }
            }

            if stripped.hasPrefix("[") {
                guard stripped.hasSuffix("]"), stripped.count > 2 else {
                    throw TOMLParseError(line: lineNumber, message: "malformed table header")
                }
                let name = String(stripped.dropFirst().dropLast())
                    .trimmingCharacters(in: .whitespaces)
                guard !name.isEmpty, !name.contains("["), !name.contains(".") else {
                    throw TOMLParseError(line: lineNumber, message: "unsupported table header")
                }
                currentTable = name
                if root[name] == nil { root[name] = .table([:]) }
                continue
            }

            guard let equals = stripped.firstIndex(of: "=") else {
                throw TOMLParseError(line: lineNumber, message: "expected '=' after key")
            }
            let key = String(stripped[..<equals]).trimmingCharacters(in: .whitespaces)
            guard !key.isEmpty, key.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "_" || $0 == "-" })
            else {
                throw TOMLParseError(line: lineNumber, message: "unsupported key \(key)")
            }
            var scanner = ValueScanner(
                text: String(stripped[stripped.index(after: equals)...]), line: lineNumber)
            let value = try scanner.parseValue()
            scanner.skipWhitespace()
            guard scanner.isAtEnd else {
                throw TOMLParseError(line: lineNumber, message: "unexpected trailing content")
            }

            if let currentTable {
                guard case .table(var table)? = root[currentTable] else {
                    throw TOMLParseError(line: lineNumber, message: "corrupt table state")
                }
                table[key] = value
                root[currentTable] = .table(table)
            } else {
                root[key] = value
            }
        }
        return root
    }

    private static func bracketBalance(_ text: String) -> Int {
        var balance = 0
        var inString = false
        var escaped = false
        for character in text {
            if escaped {
                escaped = false
                continue
            }
            if inString && character == "\\" {
                escaped = true
                continue
            }
            if character == "\"" {
                inString.toggle()
                continue
            }
            if inString { continue }
            if character == "[" { balance += 1 }
            if character == "]" { balance -= 1 }
        }
        return balance
    }

    private static func stripComment(_ line: String, line lineNumber: Int) throws -> String {
        var inString = false
        var escaped = false
        var result = ""
        for character in line {
            if escaped {
                escaped = false
                result.append(character)
                continue
            }
            if inString && character == "\\" {
                escaped = true
                result.append(character)
                continue
            }
            if character == "\"" {
                inString.toggle()
                result.append(character)
                continue
            }
            if character == "#" && !inString {
                return result
            }
            result.append(character)
        }
        guard !inString else {
            throw TOMLParseError(line: lineNumber, message: "unterminated string")
        }
        return result
    }

    private struct ValueScanner {
        let characters: [Character]
        var position = 0
        let line: Int

        init(text: String, line: Int) {
            self.characters = Array(text)
            self.line = line
        }

        var isAtEnd: Bool { position >= characters.count }

        mutating func skipWhitespace() {
            while position < characters.count, characters[position].isWhitespace {
                position += 1
            }
        }

        mutating func parseValue() throws -> TOMLValue {
            skipWhitespace()
            guard !isAtEnd else {
                throw TOMLParseError(line: line, message: "expected a value")
            }
            switch characters[position] {
            case "\"":
                return .string(try parseString())
            case "[":
                return try parseArray()
            case "{":
                return try parseInlineTable()
            default:
                return try parseScalarWord()
            }
        }

        mutating func parseString() throws -> String {
            position += 1
            var result = ""
            while position < characters.count {
                let character = characters[position]
                position += 1
                if character == "\\" {
                    guard position < characters.count else { break }
                    let escapeCharacter = characters[position]
                    position += 1
                    switch escapeCharacter {
                    case "\"": result.append("\"")
                    case "\\": result.append("\\")
                    case "n": result.append("\n")
                    case "t": result.append("\t")
                    default:
                        throw TOMLParseError(
                            line: line, message: "unsupported escape \\\(escapeCharacter)")
                    }
                    continue
                }
                if character == "\"" {
                    return result
                }
                result.append(character)
            }
            throw TOMLParseError(line: line, message: "unterminated string")
        }

        mutating func parseArray() throws -> TOMLValue {
            position += 1
            var values: [TOMLValue] = []
            while true {
                skipWhitespace()
                guard !isAtEnd else {
                    throw TOMLParseError(line: line, message: "unterminated array")
                }
                if characters[position] == "]" {
                    position += 1
                    return .array(values)
                }
                values.append(try parseValue())
                skipWhitespace()
                guard !isAtEnd else {
                    throw TOMLParseError(line: line, message: "unterminated array")
                }
                if characters[position] == "," {
                    position += 1
                } else if characters[position] != "]" {
                    throw TOMLParseError(line: line, message: "expected ',' or ']' in array")
                }
            }
        }

        mutating func parseInlineTable() throws -> TOMLValue {
            position += 1
            var table: [String: TOMLValue] = [:]
            while true {
                skipWhitespace()
                guard !isAtEnd else {
                    throw TOMLParseError(line: line, message: "unterminated inline table")
                }
                if characters[position] == "}" {
                    position += 1
                    return .table(table)
                }
                var key = ""
                while position < characters.count,
                    characters[position].isLetter || characters[position].isNumber
                        || characters[position] == "_" || characters[position] == "-"
                {
                    key.append(characters[position])
                    position += 1
                }
                guard !key.isEmpty else {
                    throw TOMLParseError(line: line, message: "expected a key in inline table")
                }
                skipWhitespace()
                guard !isAtEnd, characters[position] == "=" else {
                    throw TOMLParseError(line: line, message: "expected '=' in inline table")
                }
                position += 1
                table[key] = try parseValue()
                skipWhitespace()
                guard !isAtEnd else {
                    throw TOMLParseError(line: line, message: "unterminated inline table")
                }
                if characters[position] == "," {
                    position += 1
                } else if characters[position] != "}" {
                    throw TOMLParseError(
                        line: line, message: "expected ',' or '}' in inline table")
                }
            }
        }

        mutating func parseScalarWord() throws -> TOMLValue {
            var word = ""
            while position < characters.count, !characters[position].isWhitespace,
                characters[position] != ",", characters[position] != "]",
                characters[position] != "}"
            {
                word.append(characters[position])
                position += 1
            }
            if word == "true" { return .bool(true) }
            if word == "false" { return .bool(false) }
            if let integer = Int(word) { return .int(integer) }
            throw TOMLParseError(line: line, message: "unsupported value \(word)")
        }
    }
}
