actor ScanTurnstile {
    private var tail: Task<Void, Never>?

    func run<T: Sendable>(
        _ operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        let previous = tail
        let work = Task { [previous] () throws -> T in
            await previous?.value
            return try await operation()
        }
        tail = Task { _ = try? await work.value }
        return try await work.value
    }
}
