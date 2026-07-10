struct ThinkSplitter: Sendable {
    enum Piece: Sendable, Hashable {
        case text(String)
        case thinking(String)
    }

    private enum Mode: Sendable {
        case text
        case thinking
    }

    private static let openTag = "<think>"
    private static let closeTag = "</think>"

    private var mode: Mode = .text
    private var buffer = ""

    init() {}

    mutating func feed(_ chunk: String) -> [Piece] {
        buffer += chunk
        var output: [Piece] = []
        outer: while true {
            switch mode {
            case .text:
                guard let range = buffer.range(of: Self.openTag) else {
                    let emit = Self.emittablePrefix(of: buffer, holdingFor: Self.openTag)
                    if !emit.isEmpty {
                        output.append(.text(emit))
                        buffer.removeFirst(emit.count)
                    }
                    break outer
                }
                let before = String(buffer[buffer.startIndex..<range.lowerBound])
                if !before.isEmpty { output.append(.text(before)) }
                buffer.removeSubrange(buffer.startIndex..<range.upperBound)
                mode = .thinking
            case .thinking:
                guard let range = buffer.range(of: Self.closeTag) else {
                    let emit = Self.emittablePrefix(of: buffer, holdingFor: Self.closeTag)
                    if !emit.isEmpty {
                        output.append(.thinking(emit))
                        buffer.removeFirst(emit.count)
                    }
                    break outer
                }
                let before = String(buffer[buffer.startIndex..<range.lowerBound])
                if !before.isEmpty { output.append(.thinking(before)) }
                buffer.removeSubrange(buffer.startIndex..<range.upperBound)
                mode = .text
            }
        }
        return output
    }

    mutating func flush() -> [Piece] {
        guard !buffer.isEmpty else { return [] }
        let piece: Piece = mode == .thinking ? .thinking(buffer) : .text(buffer)
        buffer = ""
        return [piece]
    }

    private static func emittablePrefix(of buffer: String, holdingFor tag: String) -> String {
        let maxHold = Swift.min(tag.count - 1, buffer.count)
        guard maxHold > 0 else { return buffer }
        for length in stride(from: maxHold, through: 1, by: -1) {
            let suffix = String(buffer.suffix(length))
            if tag.hasPrefix(suffix) {
                return String(buffer.dropLast(length))
            }
        }
        return buffer
    }
}
