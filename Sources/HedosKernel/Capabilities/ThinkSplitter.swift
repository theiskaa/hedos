struct ThinkSplitter: Sendable {
    enum Piece: Sendable, Hashable {
        case text(String)
        case thinking(String)
    }

    struct TagPair: Sendable, Hashable {
        let open: String
        let close: String
    }

    static let defaultPairs: [TagPair] = [
        TagPair(open: "<think>", close: "</think>"),
        TagPair(open: "<|START_THINKING|>", close: "<|END_THINKING|>"),
    ]

    private enum Mode: Sendable {
        case text
        case thinking(close: String)
    }

    private let pairs: [TagPair]
    private let openTags: [String]
    private var mode: Mode = .text
    private var buffer = ""

    init(pairs: [TagPair] = defaultPairs) {
        self.pairs = pairs
        self.openTags = pairs.map(\.open)
    }

    mutating func feed(_ chunk: String) -> [Piece] {
        buffer += chunk
        var output: [Piece] = []
        outer: while true {
            switch mode {
            case .text:
                var earliest: (range: Range<String.Index>, close: String)?
                for pair in pairs {
                    guard let range = buffer.range(of: pair.open) else { continue }
                    if earliest == nil || range.lowerBound < earliest!.range.lowerBound {
                        earliest = (range, pair.close)
                    }
                }
                guard let found = earliest else {
                    let emit = Self.emittablePrefix(of: buffer, holdingForAny: openTags)
                    if !emit.isEmpty {
                        output.append(.text(emit))
                        buffer.removeFirst(emit.count)
                    }
                    break outer
                }
                let before = String(buffer[buffer.startIndex..<found.range.lowerBound])
                if !before.isEmpty { output.append(.text(before)) }
                buffer.removeSubrange(buffer.startIndex..<found.range.upperBound)
                mode = .thinking(close: found.close)
            case .thinking(let close):
                guard let range = buffer.range(of: close) else {
                    let emit = Self.emittablePrefix(of: buffer, holdingForAny: [close])
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
        let piece: Piece
        switch mode {
        case .thinking: piece = .thinking(buffer)
        case .text: piece = .text(buffer)
        }
        buffer = ""
        return [piece]
    }

    private static func emittablePrefix(of buffer: String, holdingForAny tags: [String]) -> String {
        let longest = tags.map(\.count).max() ?? 1
        let maxHold = Swift.min(longest - 1, buffer.count)
        guard maxHold > 0 else { return buffer }
        for length in stride(from: maxHold, through: 1, by: -1) {
            let suffix = String(buffer.suffix(length))
            if tags.contains(where: { $0.hasPrefix(suffix) }) {
                return String(buffer.dropLast(length))
            }
        }
        return buffer
    }

    static func hasVisibleTags(in text: String) -> Bool {
        for pair in defaultPairs where text.contains(pair.open) || text.contains(pair.close) {
            return true
        }
        return false
    }

    static func separating(
        _ upstream: AsyncThrowingStream<CapabilityChunk, Error>
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                var splitter = ThinkSplitter()
                func drain(_ pieces: [Piece]) {
                    for piece in pieces {
                        switch piece {
                        case .text(let value): continuation.yield(.text(value))
                        case .thinking(let value): continuation.yield(.thinking(value))
                        }
                    }
                }
                do {
                    for try await chunk in upstream {
                        switch chunk {
                        case .text(let text):
                            drain(splitter.feed(text))
                        case .done(let stats):
                            drain(splitter.flush())
                            continuation.yield(.done(stats))
                        default:
                            continuation.yield(chunk)
                        }
                    }
                    drain(splitter.flush())
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
