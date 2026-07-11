import Foundation

public enum SweepStatus: String, Sendable, Hashable {
    case pass
    case fail
    case skip
}

public struct SweepParity: Sendable, Hashable {
    public var thinkingSeparated: Bool
    public var templateNoticeFired: Bool
    public var promptCompleteOK: Bool
    public var statsReported: Bool

    public init(
        thinkingSeparated: Bool, templateNoticeFired: Bool, promptCompleteOK: Bool,
        statsReported: Bool
    ) {
        self.thinkingSeparated = thinkingSeparated
        self.templateNoticeFired = templateNoticeFired
        self.promptCompleteOK = promptCompleteOK
        self.statsReported = statsReported
    }

    public var column: String {
        func mark(_ ok: Bool) -> String { ok ? "ok" : "no" }
        return
            "think:\(mark(thinkingSeparated)) notice:\(templateNoticeFired ? "fired" : "none") "
            + "complete:\(mark(promptCompleteOK)) stats:\(mark(statsReported))"
    }
}

public struct SweepResult: Sendable, Hashable {
    public var model: String
    public var capability: Capability?
    public var status: SweepStatus
    public var durationMs: Int
    public var reason: String?
    public var parity: SweepParity?

    public init(
        model: String,
        capability: Capability?,
        status: SweepStatus,
        durationMs: Int,
        reason: String? = nil,
        parity: SweepParity? = nil
    ) {
        self.model = model
        self.capability = capability
        self.status = status
        self.durationMs = durationMs
        self.reason = reason
        self.parity = parity
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
            result.parity?.column ?? "-",
            result.reason ?? "-",
        ]
        return fields.joined(separator: " · ")
    }
}
