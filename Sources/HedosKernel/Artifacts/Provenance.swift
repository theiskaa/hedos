import Foundation

public enum Provenance {
    public static func line(for artifact: Artifact, schema: [ParamSpec] = []) -> String {
        var parts = [artifact.model]
        parts.append(
            contentsOf: paramPairs(artifact.params, schema: schema).map { "\($0.key) \($0.value)" })
        parts.append(duration(ms: artifact.durationMs))
        return parts.joined(separator: " · ")
    }

    public static func duration(ms: Int) -> String {
        if ms < 1000 {
            return "\(ms) ms"
        }
        if ms < 60_000 {
            return String(format: "%.1fs", Double(ms) / 1000)
        }
        let minutes = ms / 60_000
        let seconds = (ms % 60_000) / 1000
        return seconds == 0 ? "\(minutes)m" : "\(minutes)m \(seconds)s"
    }

    public static func details(for artifact: Artifact, schema: [ParamSpec] = []) -> String {
        var lines = [
            "model: \(artifact.model)",
            "runtime: \(artifact.runtime)",
            "capability: \(artifact.capability.rawValue)",
        ]
        lines.append(contentsOf: promptAndParamLines(artifact.params, schema: schema))
        lines.append("duration: \(duration(ms: artifact.durationMs))")
        lines.append("job: \(artifact.jobID)")
        return lines.joined(separator: "\n")
    }

    public static func failureDetails(
        model: String, error: String, jobID: String?, params: JSONValue, schema: [ParamSpec] = []
    ) -> String {
        var lines = ["model: \(model)", "error: \(error)"]
        if let jobID {
            lines.append("job: \(jobID)")
        }
        lines.append(contentsOf: promptAndParamLines(params, schema: schema))
        return lines.joined(separator: "\n")
    }

    public static func prompt(of params: JSONValue) -> String? {
        guard case .object(let fields) = params, case .string(let prompt) = fields["prompt"]
        else { return nil }
        return prompt
    }

    private static func promptAndParamLines(_ params: JSONValue, schema: [ParamSpec]) -> [String] {
        var lines: [String] = []
        if let prompt = prompt(of: params) {
            lines.append("prompt: \(prompt)")
        }
        lines.append(
            contentsOf: paramPairs(params, schema: schema).map { "\($0.key): \($0.value)" })
        return lines
    }

    static func paramPairs(
        _ params: JSONValue, schema: [ParamSpec]
    ) -> [(key: String, value: String)] {
        guard case .object(let fields) = params else { return [] }
        var keys = schema.map(\.key).filter { fields[$0] != nil }
        let extras = fields.keys
            .filter { $0 != "prompt" && !keys.contains($0) }
            .sorted()
        keys.append(contentsOf: extras)
        return keys.compactMap { key in
            guard key != "prompt", let value = fields[key], let rendered = scalar(value) else {
                return nil
            }
            return (key, rendered)
        }
    }

    static func scalar(_ value: JSONValue) -> String? {
        switch value {
        case .int(let raw): String(raw)
        case .double(let raw): String(format: "%g", raw)
        case .string(let raw): raw
        case .bool(let raw): String(raw)
        default: nil
        }
    }
}
