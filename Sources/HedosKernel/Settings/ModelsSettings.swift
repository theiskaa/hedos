import Foundation

public struct ModelsSettings: SettingsDomain {
    public static let domainName = "models"

    public var watchedFolders: [String]
    public var hfCacheRoots: [String]
    public var approvedNetworkRuntimes: [String]
    public var approvedNetworkRuntimeHashes: [String: String]
    public var keepWarm: KeepWarmPolicy
    public var eviction: EvictionPolicy
    public var ramBudgetMB: Int?

    public init() {
        watchedFolders = []
        hfCacheRoots = []
        approvedNetworkRuntimes = []
        approvedNetworkRuntimeHashes = [:]
        keepWarm = .fiveMinutes
        eviction = .strictSingle
        ramBudgetMB = nil
    }

    public init(
        watchedFolders: [String] = [],
        hfCacheRoots: [String] = [],
        approvedNetworkRuntimes: [String] = [],
        approvedNetworkRuntimeHashes: [String: String] = [:],
        keepWarm: KeepWarmPolicy = .fiveMinutes,
        eviction: EvictionPolicy = .strictSingle,
        ramBudgetMB: Int? = nil
    ) {
        self.watchedFolders = watchedFolders
        self.hfCacheRoots = hfCacheRoots
        self.approvedNetworkRuntimes = approvedNetworkRuntimes
        self.approvedNetworkRuntimeHashes = approvedNetworkRuntimeHashes
        self.keepWarm = keepWarm
        self.eviction = eviction
        self.ramBudgetMB = ramBudgetMB
    }

    public var residencyPolicy: ResidencyPolicy {
        ResidencyPolicy(keepWarm: keepWarm, eviction: eviction, ramBudgetMB: ramBudgetMB)
    }

    enum CodingKeys: String, CodingKey {
        case watchedFolders, hfCacheRoots, approvedNetworkRuntimes, approvedNetworkRuntimeHashes
        case keepWarm, eviction
        case ramBudgetMB
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        watchedFolders = container.lenient(
            [String].self, .watchedFolders, fallback: defaults.watchedFolders)
        hfCacheRoots = container.lenient(
            [String].self, .hfCacheRoots, fallback: defaults.hfCacheRoots)
        approvedNetworkRuntimes = container.lenient(
            [String].self, .approvedNetworkRuntimes,
            fallback: defaults.approvedNetworkRuntimes)
        approvedNetworkRuntimeHashes = container.lenient(
            [String: String].self, .approvedNetworkRuntimeHashes,
            fallback: defaults.approvedNetworkRuntimeHashes)
        keepWarm = container.lenient(KeepWarmPolicy.self, .keepWarm, fallback: defaults.keepWarm)
        eviction = container.lenient(EvictionPolicy.self, .eviction, fallback: defaults.eviction)
        ramBudgetMB = container.lenient(Int.self, .ramBudgetMB)
    }

    public static func compatibilityRead(from directory: URL) -> ModelsSettings? {
        let legacy = directory.appendingPathComponent("settings.json")
        guard let data = try? Data(contentsOf: legacy),
            let value = try? JSONDecoder().decode(JSONValue.self, from: data),
            case .object(let fields) = value,
            case .array(let folders)? = fields["watchedFolders"]
        else { return nil }
        var settings = ModelsSettings()
        settings.watchedFolders = folders.compactMap {
            guard case .string(let path) = $0 else { return nil }
            return path
        }
        return settings
    }
}
