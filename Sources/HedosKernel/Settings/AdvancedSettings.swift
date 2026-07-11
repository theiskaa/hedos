import Foundation

public struct AdvancedSettings: SettingsDomain {
    public static let domainName = "advanced"

    public var jobHistoryLimit: Int

    public init() {
        jobHistoryLimit = 50
    }

    public init(jobHistoryLimit: Int = 50) {
        self.jobHistoryLimit = jobHistoryLimit
    }

    enum CodingKeys: String, CodingKey {
        case jobHistoryLimit
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        jobHistoryLimit = container.lenient(
            Int.self, .jobHistoryLimit, fallback: defaults.jobHistoryLimit)
    }
}
