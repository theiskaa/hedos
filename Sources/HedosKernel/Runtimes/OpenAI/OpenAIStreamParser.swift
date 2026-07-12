import Foundation

struct OpenAIStreamParser {
    private let started = ContinuousClock.now
    private var promptTokens: Int?
    private var completionTokens: Int?
    private var finishReason: String?
    private var toolFragments: [Int: (id: String?, name: String, arguments: String)] = [:]

    mutating func parse(line: String) -> [CapabilityChunk] {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        guard trimmed.hasPrefix("data:") else { return [] }
        let payload = trimmed.dropFirst("data:".count).trimmingCharacters(in: .whitespaces)
        if payload == "[DONE]" {
            let durationMs = Int((ContinuousClock.now - started) / .milliseconds(1))
            var chunks = flushToolCalls()
            chunks.append(
                .done(
                    GenerationStats(
                        promptTokens: promptTokens,
                        completionTokens: completionTokens,
                        durationMs: durationMs,
                        finishReason: finishReason)))
            return chunks
        }
        guard let data = payload.data(using: .utf8),
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return [] }

        if let usage = object["usage"] as? [String: Any] {
            promptTokens = usage["prompt_tokens"] as? Int ?? promptTokens
            completionTokens = usage["completion_tokens"] as? Int ?? completionTokens
        }

        var chunks: [CapabilityChunk] = []
        if let choices = object["choices"] as? [[String: Any]], let choice = choices.first {
            if let delta = choice["delta"] as? [String: Any] {
                if let reasoning = delta["reasoning_content"] as? String, !reasoning.isEmpty {
                    chunks.append(.thinking(reasoning))
                }
                if let content = delta["content"] as? String, !content.isEmpty {
                    chunks.append(.text(content))
                }
                if let calls = delta["tool_calls"] as? [[String: Any]] {
                    for entry in calls {
                        accumulate(entry)
                    }
                }
            }
            if let reason = choice["finish_reason"] as? String, !reason.isEmpty {
                finishReason = reason
                chunks.append(contentsOf: flushToolCalls())
            }
        }
        return chunks
    }

    private mutating func accumulate(_ entry: [String: Any]) {
        let index = entry["index"] as? Int ?? 0
        var fragment = toolFragments[index] ?? (id: nil, name: "", arguments: "")
        if let id = entry["id"] as? String, !id.isEmpty { fragment.id = id }
        if let function = entry["function"] as? [String: Any] {
            if let name = function["name"] as? String, !name.isEmpty {
                fragment.name += name
            }
            if let arguments = function["arguments"] as? String {
                fragment.arguments += arguments
            }
        }
        toolFragments[index] = fragment
    }

    private mutating func flushToolCalls() -> [CapabilityChunk] {
        guard !toolFragments.isEmpty else { return [] }
        let fragments = toolFragments.sorted { $0.key < $1.key }.map(\.value)
        toolFragments = [:]
        return fragments.compactMap { fragment in
            guard !fragment.name.isEmpty else { return nil }
            let raw = fragment.arguments.trimmingCharacters(in: .whitespacesAndNewlines)
            let arguments: JSONValue
            if let data = fragment.arguments.data(using: .utf8),
                let parsed = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                let value = JSONValue.fromAny(parsed)
            {
                arguments = value
            } else if raw.isEmpty {
                arguments = .object([:])
            } else {
                arguments = .object(["_raw": .string(fragment.arguments)])
            }
            let call =
                fragment.id.map {
                    ToolCall(id: $0, name: fragment.name, arguments: arguments)
                } ?? ToolCall(name: fragment.name, arguments: arguments)
            return .toolCall(call)
        }
    }
}
