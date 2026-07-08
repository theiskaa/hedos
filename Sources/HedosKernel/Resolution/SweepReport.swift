import Foundation

public enum SweepStatus: String, Sendable, Hashable {
    case pass
    case fail
    case skip
}

public struct SweepResult: Sendable, Hashable {
    public var model: String
    public var capability: Capability?
    public var status: SweepStatus
    public var durationMs: Int
    public var reason: String?

    public init(
        model: String,
        capability: Capability?,
        status: SweepStatus,
        durationMs: Int,
        reason: String? = nil
    ) {
        self.model = model
        self.capability = capability
        self.status = status
        self.durationMs = durationMs
        self.reason = reason
    }
}

public enum SweepReport {
    public static func render(_ results: [SweepResult]) -> String {
        results.map(line).joined(separator: "\n")
    }

    static func line(_ result: SweepResult) -> String {
        let fields = [
            result.model,
            result.capability?.rawValue ?? "-",
            result.status.rawValue,
            "\(result.durationMs)ms",
            result.reason ?? "-",
        ]
        return fields.joined(separator: " · ")
    }
}
