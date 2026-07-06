public enum KeepWarmPolicy: String, Codable, Sendable, CaseIterable {
    case fiveMinutes = "5min"
    case fifteenMinutes = "15min"
    case oneHour = "1h"
    case never

    public var warmWindow: Duration {
        switch self {
        case .fiveMinutes: .seconds(300)
        case .fifteenMinutes: .seconds(900)
        case .oneHour: .seconds(3600)
        case .never: .zero
        }
    }
}

public enum EvictionPolicy: String, Codable, Sendable, CaseIterable {
    case strictSingle
    case budgeted
}

public struct ResidencyPolicy: Sendable, Equatable {
    public var keepWarm: KeepWarmPolicy
    public var eviction: EvictionPolicy
    public var ramBudgetMB: Int?

    public init(
        keepWarm: KeepWarmPolicy = .fiveMinutes,
        eviction: EvictionPolicy = .strictSingle,
        ramBudgetMB: Int? = nil
    ) {
        self.keepWarm = keepWarm
        self.eviction = eviction
        self.ramBudgetMB = ramBudgetMB
    }
}
