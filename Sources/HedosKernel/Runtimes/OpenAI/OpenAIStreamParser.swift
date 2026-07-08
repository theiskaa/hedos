import Foundation

struct OpenAIStreamParser {
    private let started = ContinuousClock.now
    private var promptTokens: Int?
    private var completionTokens: Int?

    mutating func parse(line: String) -> [CapabilityChunk] {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix("data:") else { return [] }
        let payload = trimmed.dropFirst("data:".count).trimmingCharacters(in: .whitespaces)
        if payload == "[DONE]" {
            let durationMs = Int((ContinuousClock.now - started) / .milliseconds(1))
            return [
                .done(
                    GenerationStats(
                        promptTokens: promptTokens,
                        completionTokens: completionTokens,
                        durationMs: durationMs))
            ]
        }
        guard let data = payload.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return [] }

        if let usage = object["usage"] as? [String: Any] {
            promptTokens = usage["prompt_tokens"] as? Int ?? promptTokens
            completionTokens = usage["completion_tokens"] as? Int ?? completionTokens
        }

        var chunks: [CapabilityChunk] = []
        if let choices = object["choices"] as? [[String: Any]],
            let delta = choices.first?["delta"] as? [String: Any]
        {
            if let reasoning = delta["reasoning_content"] as? String, !reasoning.isEmpty {
                chunks.append(.thinking(reasoning))
            }
            if let content = delta["content"] as? String, !content.isEmpty {
                chunks.append(.text(content))
            }
        }
        return chunks
    }
}
