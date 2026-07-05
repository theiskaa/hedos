import Foundation

public enum OllamaStreamParser {
    public static func parse(line: String) -> CapabilityChunk? {
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
