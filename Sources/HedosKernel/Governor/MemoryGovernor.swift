import Foundation

public enum RAMVerdict: String, Sendable {
    case ok
    case tight
}

public struct ResidentModel: Hashable, Sendable {
    public let modelID: String
    public let name: String
    public var footprintMB: Int
    public let loadedAt: Date

    public init(modelID: String, name: String, footprintMB: Int, loadedAt: Date = Date()) {
        self.modelID = modelID
        self.name = name
        self.footprintMB = footprintMB
        self.loadedAt = loadedAt
    }
}

public actor MemoryGovernor {
    public static let shared = MemoryGovernor()

    public nonisolated let gate = GPUGate()
    public nonisolated let leases = ModelLease()
    public nonisolated let residency: ResidencyManager

    private let totalMemoryMB: Int
    private let heavyThresholdMB: Int
    private let tightFraction: Double
    private var residents: [String: ResidentModel] = [:]
    private var evictionPolicy: EvictionPolicy = .strictSingle
    private var ramBudgetMB: Int?

    public init(
        totalMemoryMB: Int = Int(ProcessInfo.processInfo.physicalMemory / (1 << 20)),
        heavyThresholdMB: Int = 1024,
        tightFraction: Double = 0.8,
        defaultWarmWindow: Duration = .seconds(120)
    ) {
        self.totalMemoryMB = totalMemoryMB
        self.heavyThresholdMB = heavyThresholdMB
        self.tightFraction = tightFraction
        self.residency = ResidencyManager(defaultWarmWindow: defaultWarmWindow)
    }

    public var defaultBudgetMB: Int {
        Int(Double(totalMemoryMB) * tightFraction)
    }

    @discardableResult
    public func admit(
        modelID: String,
        name: String,
        footprintMB: Int?,
        onWait: (@Sendable (String) async -> Void)? = nil
    ) async throws -> RAMVerdict {
        if isHeavy(footprintMB) {
            while let conflict = evictionConflict(admitting: modelID, footprintMB: footprintMB) {
                if await leases.count(conflict.modelID) > 0 {
                    await onWait?("Waiting for \(conflict.name) to finish")
                    try await leases.drain(conflict.modelID)
                }
                try Task.checkCancellation()
                await residency.unloadNow(conflict.modelID)
            }
        }
        let verdict = verdict(admitting: footprintMB, for: modelID)
        await reserve(modelID: modelID, name: name, footprintMB: footprintMB)
        return verdict
    }

    public func markLoaded(
        modelID: String,
        name: String,
        footprintMB: Int?,
        warmWindow: Duration? = nil,
        unloader: @escaping @Sendable () async -> Void
    ) async {
        residents[modelID] = ResidentModel(
            modelID: modelID,
            name: name,
            footprintMB: footprintMB ?? heavyThresholdMB)
        await registerGovernedUnloader(modelID, warmWindow: warmWindow, unloader: unloader)
    }

    private func reserve(modelID: String, name: String, footprintMB: Int?) async {
        guard residents[modelID] == nil else { return }
        residents[modelID] = ResidentModel(
            modelID: modelID,
            name: name,
            footprintMB: footprintMB ?? heavyThresholdMB)
        await registerGovernedUnloader(modelID) {}
    }

    private func registerGovernedUnloader(
        _ modelID: String,
        warmWindow: Duration? = nil,
        unloader: @escaping @Sendable () async -> Void
    ) async {
        let gate = gate
        let leases = leases
        await residency.register(modelID, warmWindow: warmWindow) { [weak self] in
            await gate.withAccess(.unload(modelID: modelID)) { [weak self] in
                if await leases.count(modelID) > 0 { return false }
                await unloader()
                await self?.markUnloaded(modelID)
                return true
            }
        }
    }

    public func markUnloaded(_ modelID: String) {
        residents.removeValue(forKey: modelID)
    }

    public func beginGeneration(_ modelID: String) async {
        await leases.acquire(modelID)
        await residency.cancelIdleUnload(modelID)
    }

    public func endGeneration(_ modelID: String) async {
        await leases.release(modelID)
        if await leases.count(modelID) == 0, residents[modelID] != nil {
            await residency.scheduleIdleUnload(modelID)
        }
    }

    public func observeFootprint(_ modelID: String, footprintMB: Int) {
        guard var resident = residents[modelID] else { return }
        resident.footprintMB = footprintMB
        residents[modelID] = resident
    }

    public func verdict(admitting footprintMB: Int?, for modelID: String? = nil) -> RAMVerdict {
        let incoming = footprintMB ?? heavyThresholdMB
        let residentFootprint = residents.values
            .filter { $0.modelID != modelID }
            .reduce(0) { $0 + $1.footprintMB }
        let ceiling = Double(totalMemoryMB) * tightFraction
        return Double(residentFootprint + incoming) > ceiling ? .tight : .ok
    }

    public func resident() -> [ResidentModel] {
        residents.values.sorted { ($0.loadedAt, $0.modelID) < ($1.loadedAt, $1.modelID) }
    }

    public func isResident(_ modelID: String) -> Bool {
        residents[modelID] != nil
    }

    public func setWarmWindow(_ window: Duration, for modelID: String) async {
        await residency.setWarmWindow(window, for: modelID)
    }

    public func apply(policy: ResidencyPolicy) async {
        evictionPolicy = policy.eviction
        ramBudgetMB = policy.ramBudgetMB
        await residency.setDefaultWarmWindow(policy.keepWarm.warmWindow)
    }

    public func currentEvictionPolicy() -> EvictionPolicy {
        evictionPolicy
    }

    public func suspendForQuit() async {
        await residency.suspendAll()
    }

    public func wouldWait(admitting modelID: String, footprintMB: Int?) async -> Bool {
        guard isHeavy(footprintMB) else { return false }
        guard let conflict = evictionConflict(admitting: modelID, footprintMB: footprintMB)
        else { return false }
        return await leases.count(conflict.modelID) > 0
    }

    private func isHeavy(_ footprintMB: Int?) -> Bool {
        (footprintMB ?? heavyThresholdMB) >= heavyThresholdMB
    }

    private func heavyResident(besides modelID: String) -> ResidentModel? {
        residents.values.first { $0.modelID != modelID && $0.footprintMB >= heavyThresholdMB }
    }

    private func evictionConflict(
        admitting modelID: String, footprintMB: Int?
    ) -> ResidentModel? {
        switch evictionPolicy {
        case .strictSingle:
            return heavyResident(besides: modelID)
        case .budgeted:
            let budget = ramBudgetMB ?? Int(Double(totalMemoryMB) * tightFraction)
            let incoming = footprintMB ?? heavyThresholdMB
            let others = residents.values.filter { $0.modelID != modelID }
            let occupied = others.reduce(0) { $0 + $1.footprintMB }
            guard occupied + incoming > budget else { return nil }
            return
                others
                .filter { $0.footprintMB >= heavyThresholdMB }
                .min { ($0.loadedAt, $0.modelID) < ($1.loadedAt, $1.modelID) }
        }
    }
}
