public protocol JobAdmission: Sendable {
    func admit(
        _ job: Job, onWait: @escaping @Sendable (String) async -> Void
    ) async throws -> RAMVerdict
}

public struct ImmediateAdmission: JobAdmission {
    public init() {}

    public func admit(
        _ job: Job, onWait: @escaping @Sendable (String) async -> Void
    ) async throws -> RAMVerdict {
        try Task.checkCancellation()
        return .ok
    }
}
