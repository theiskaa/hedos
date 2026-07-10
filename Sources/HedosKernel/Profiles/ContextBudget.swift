import Foundation

public enum ContextBudget {
    public static let completionFloor = 256

    public enum Verdict: Equatable, Sendable {
        case fits(clampedMaxTokens: Int?)
        case exceeds(estimated: Int, window: Int)
    }

    public static func estimatedTokens(characters: Int) -> Int {
        (characters + 3) / 4
    }

    public static func effectiveWindow(
        for record: ModelRecord, requestedContextLength: Int? = nil
    ) -> Int? {
        if record.source.kind == .builtin { return 4096 }
        let window: Int?
        switch record.runtime.id {
        case "llama-cpp":
            window = LlamaCppAdapter.effectiveContextTokens(
                record: record, requested: requestedContextLength)
        case "ollama":
            window = requestedContextLength ?? record.contextLength
        case "mlx-swift", "python:mlx-lm":
            window = record.contextLength
        default:
            window = nil
        }
        guard let window, window > 0 else { return nil }
        return window
    }

    public static func storedContextLength(of record: ModelRecord) -> Int? {
        record.normalizedParamValues()["context_length"]?.intValue
    }

    public static func assess(
        promptCharacters: Int, window: Int, requestedMaxTokens: Int?
    ) -> Verdict {
        let estimated = estimatedTokens(characters: promptCharacters)
        guard estimated + completionFloor <= window else {
            return .exceeds(estimated: estimated, window: window)
        }
        let available = window - estimated
        return .fits(clampedMaxTokens: min(requestedMaxTokens ?? available, available))
    }

    public static func promptCharacters(of payload: JSONValue) -> Int {
        guard case .object(let object) = payload else { return 0 }
        var total = 0
        if case .array(let messages)? = object["messages"] {
            for message in messages {
                if case .object(let fields) = message,
                    case .string(let content)? = fields["content"]
                {
                    total += content.count
                }
            }
        }
        if case .string(let prompt)? = object["prompt"] {
            total += prompt.count
        }
        return total
    }
}
