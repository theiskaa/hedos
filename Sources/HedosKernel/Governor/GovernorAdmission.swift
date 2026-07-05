public struct GovernorAdmission: JobAdmission {
    private let governor: MemoryGovernor
    private let registry: Registry

    public init(governor: MemoryGovernor, registry: Registry) {
        self.governor = governor
        self.registry = registry
    }

    public func admit(
        _ job: Job, onWait: @escaping @Sendable (String) async -> Void
    ) async throws -> RAMVerdict {
        let record = try? await registry.get(id: job.modelID)
        return try await governor.admit(
            modelID: job.modelID,
            name: record?.name ?? job.modelID,
            footprintMB: record?.footprintMB,
            onWait: onWait)
    }
}
