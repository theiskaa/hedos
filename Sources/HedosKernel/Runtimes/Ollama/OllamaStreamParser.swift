import Foundation

enum OllamaStreamParser {
    static func errorMessage(line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty,
            let object = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8))
                as? [String: Any],
            let message = object["error"] as? String, !message.isEmpty
        else { return nil }
        return message
    }

    static func toolCalls(line: String) -> [ToolCall] {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty,
            let object = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8))
                as? [String: Any],
            let message = object["message"] as? [String: Any],
            let entries = message["tool_calls"] as? [[String: Any]]
        else { return [] }
        return entries.compactMap { entry in
            guard let function = entry["function"] as? [String: Any],
                let name = function["name"] as? String, !name.isEmpty
            else { return nil }
            let arguments =
                (function["arguments"] as? [String: Any]).flatMap { JSONValue.fromAny($0) }
                ?? .object([:])
            return ToolCall(name: name, arguments: arguments)
        }
    }

    static func parse(line: String) -> CapabilityChunk? {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty,
            let object = try? JSONSerialization.jsonObject(with: Data(trimmed.utf8))
                as? [String: Any]
        else { return nil }

        if (object["done"] as? Bool) == true {
            var stats = GenerationStats()
            stats.promptTokens = object["prompt_eval_count"] as? Int
            stats.completionTokens = object["eval_count"] as? Int
            if let nanoseconds = object["total_duration"] as? Int {
                stats.durationMs = nanoseconds / 1_000_000
            }
            stats.finishReason = object["done_reason"] as? String
            return .done(stats)
        }
        if let message = object["message"] as? [String: Any] {
            if let content = message["content"] as? String, !content.isEmpty {
                return .text(content)
            }
            if let thinking = message["thinking"] as? String, !thinking.isEmpty {
                return .thinking(thinking)
            }
        }
        return nil
    }
}
