public protocol JobAdmission: Sendable {
    func admit(_ job: Job, onWait: @escaping @Sendable (String) async -> Void) async throws
}

public struct ImmediateAdmission: JobAdmission {
    public init() {}

    public func admit(_ job: Job, onWait: @escaping @Sendable (String) async -> Void) async throws {
        try Task.checkCancellation()
    }
}
