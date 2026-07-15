import Foundation
import Synchronization
import Testing

@testable import HedosKernel

private final class ScriptedProvider: InstallProvider, @unchecked Sendable {
    let id: InstallProviderID
    let displayName = "Scripted"
    let sourceKind: SourceKind
    let supportsSearch = false
    private let script: @Sendable (InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error>
    private let invocations = Mutex(0)

    init(
        id: InstallProviderID = InstallProviderID(rawValue: "scripted"),
        sourceKind: SourceKind = .huggingfaceCache,
        script: @escaping @Sendable (InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error>
    ) {
        self.id = id
        self.sourceKind = sourceKind
        self.script = script
    }

    var installCount: Int { invocations.withLock { $0 } }

    func availability() async -> InstallAvailability { .ready }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        throw InstallError.providerUnavailable(hint: "Scripted has no search.")
    }

    func plan(reference: String) async throws -> InstallPlan {
        InstallPlan(
            provider: id, reference: reference, displayName: reference,
            destination: "~/scripted")
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        invocations.withLock { $0 += 1 }
        return script(plan)
    }
}

private final class StreamGate: Sendable {
    private let continuation = Mutex<AsyncThrowingStream<InstallStreamEvent, Error>.Continuation?>(
        nil)

    func stream() -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { continuation in
            self.continuation.withLock { $0 = continuation }
        }
    }

    func yield(_ event: InstallStreamEvent) {
        continuation.withLock { $0 }?.yield(event)
    }

    func finish(throwing error: Error? = nil) {
        continuation.withLock { $0 }?.finish(throwing: error)
    }
}

private func makePlan(
    _ provider: InstallProviderID = InstallProviderID(rawValue: "scripted"),
    reference: String = "org/model",
    totalBytes: Int64? = nil,
    remainingBytes: Int64? = nil
) -> InstallPlan {
    InstallPlan(
        provider: provider, reference: reference, displayName: reference,
        totalBytes: totalBytes, remainingBytes: remainingBytes, destination: "~/scripted")
}

private func instantFinish() -> ScriptedProvider {
    ScriptedProvider { _ in
        AsyncThrowingStream { continuation in
            continuation.finish()
        }
    }
}

private func collect(_ stream: AsyncStream<InstallEvent>) async -> [InstallEvent] {
    var events: [InstallEvent] = []
    for await event in stream {
        events.append(event)
    }
    return events
}

private func pollUntil(
    timeout: Duration = .seconds(5), _ condition: @Sendable () async -> Bool
) async throws {
    let deadline = ContinuousClock.now + timeout
    while ContinuousClock.now < deadline {
        if await condition() { return }
        try await Task.sleep(nanoseconds: 10_000_000)
    }
    Issue.record("condition never became true within \(timeout)")
}

struct InstallServiceTests {
    @Test func installRunsToDoneWithProgressAndStatus() async throws {
        let gate = StreamGate()
        let provider = ScriptedProvider { _ in gate.stream() }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let id = try await service.begin(makePlan())
        try await pollUntil { provider.installCount == 1 }
        let stream = await service.events(id: id)
        gate.yield(.status("Connecting"))
        gate.yield(.progress(InstallProgress(bytesDownloaded: 5, totalBytes: 10)))
        gate.yield(.progress(InstallProgress(bytesDownloaded: 10, totalBytes: 10)))
        gate.finish()
        let events = await collect(stream)
        #expect(events.contains(.status("Connecting")))
        #expect(events.last == .done)
        let fractions = events.compactMap { event -> Double? in
            if case .progress(let progress) = event { return progress.fraction }
            return nil
        }
        #expect(fractions.contains(1.0))
    }

    @Test func failingProviderConcludesFailed() async throws {
        let provider = ScriptedProvider { _ in
            AsyncThrowingStream { continuation in
                continuation.finish(throwing: InstallError.transferFailed("socket closed"))
            }
        }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let id = try await service.begin(makePlan())
        try await pollUntil { await service.active().isEmpty }
        let events = await collect(await service.events(id: id))
        #expect(events == [.failed(message: "socket closed")])
    }

    @Test func beginJoinsInFlightInstallOfSameReference() async throws {
        let gate = StreamGate()
        let provider = ScriptedProvider { _ in gate.stream() }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let first = try await service.begin(makePlan())
        let second = try await service.begin(makePlan())
        #expect(first == second)
        try await pollUntil { provider.installCount == 1 }
        #expect(provider.installCount == 1)
        gate.finish()
        try await pollUntil { await service.active().isEmpty }
        let third = try await service.begin(makePlan())
        #expect(third != first)
        try await pollUntil { provider.installCount == 2 }
        gate.finish()
        try await pollUntil { await service.active().isEmpty }
    }

    @Test func eventsFanOutToMultipleSubscribersWithReplay() async throws {
        let gate = StreamGate()
        let provider = ScriptedProvider { _ in gate.stream() }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let id = try await service.begin(makePlan())
        try await pollUntil { provider.installCount == 1 }
        gate.yield(.progress(InstallProgress(bytesDownloaded: 3, totalBytes: 10)))
        try await pollUntil {
            await service.active().first?.progress.bytesDownloaded == 3
        }
        let firstStream = await service.events(id: id)
        let secondStream = await service.events(id: id)
        gate.yield(.progress(InstallProgress(bytesDownloaded: 10, totalBytes: 10)))
        gate.finish()
        let firstEvents = await collect(firstStream)
        let secondEvents = await collect(secondStream)
        for events in [firstEvents, secondEvents] {
            #expect(events.first == .preparing)
            #expect(
                events.contains(.progress(InstallProgress(bytesDownloaded: 3, totalBytes: 10))))
            #expect(
                events.contains(.progress(InstallProgress(bytesDownloaded: 10, totalBytes: 10))))
            #expect(events.last == .done)
        }
    }

    @Test func subscribingAfterCompletionYieldsTerminalEvent() async throws {
        let service = InstallService(providers: [instantFinish()], freeDiskBytes: { _ in .max })
        let id = try await service.begin(makePlan())
        try await pollUntil { await service.active().isEmpty }
        let replayed = await collect(await service.events(id: id))
        #expect(replayed == [.done])
    }

    @Test func cancelConcludesCancelledAndReleasesReference() async throws {
        let gate = StreamGate()
        let provider = ScriptedProvider { _ in gate.stream() }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let id = try await service.begin(makePlan())
        try await pollUntil { provider.installCount == 1 }
        let stream = await service.events(id: id)
        await service.cancel(id)
        let events = await collect(stream)
        #expect(events.last == .cancelled)
        try await pollUntil { await service.active().isEmpty }
        let again = try await service.begin(makePlan())
        #expect(again != id)
        try await pollUntil { provider.installCount == 2 }
        gate.finish()
        try await pollUntil { await service.active().isEmpty }
    }

    @Test func completionsYieldProviderKindOnTermination() async throws {
        let provider = ScriptedProvider(sourceKind: .ollama) { _ in
            AsyncThrowingStream { continuation in
                continuation.finish()
            }
        }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let completions = await service.completions()
        _ = try await service.begin(makePlan())
        var iterator = completions.makeAsyncIterator()
        let kinds = await iterator.next()
        #expect(kinds == [.ollama])
    }

    @Test func insufficientDiskRejectsBeforeStarting() async throws {
        let provider = instantFinish()
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in 100 })
        do {
            _ = try await service.begin(makePlan(totalBytes: 1000))
            Issue.record("begin should have thrown")
        } catch let error as InstallError {
            guard case .insufficientDisk(let required, let available) = error else {
                Issue.record("unexpected error \(error)")
                return
            }
            #expect(required == 1050)
            #expect(available == 100)
        }
        #expect(provider.installCount == 0)
    }

    @Test func resumePreflightChecksOnlyRemainingBytes() async throws {
        let service = InstallService(providers: [instantFinish()], freeDiskBytes: { _ in 200 })
        let id = try await service.begin(
            makePlan(totalBytes: 1000, remainingBytes: 100))
        try await pollUntil { await service.active().isEmpty }
        let events = await collect(await service.events(id: id))
        #expect(events.last == .done)
    }

    @Test func planWithoutTotalBytesSkipsDiskPreflight() async throws {
        let service = InstallService(providers: [instantFinish()], freeDiskBytes: { _ in 0 })
        let id = try await service.begin(makePlan())
        try await pollUntil { await service.active().isEmpty }
        let events = await collect(await service.events(id: id))
        #expect(events == [.done])
    }

    @Test func unknownProviderThrows() async throws {
        let service = InstallService(providers: [], freeDiskBytes: { _ in .max })
        await #expect(throws: InstallError.providerUnknown(InstallProviderID(rawValue: "nope"))) {
            _ = try await service.plan(
                provider: InstallProviderID(rawValue: "nope"), reference: "x")
        }
    }

    @Test func unavailableProviderRefusesPlanAndSearch() async throws {
        struct DownProvider: InstallProvider {
            let id = InstallProviderID(rawValue: "down")
            let displayName = "Down"
            let sourceKind = SourceKind.ollama
            let supportsSearch = true
            func availability() async -> InstallAvailability {
                .unavailable(hint: "Down isn't installed.")
            }
            func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
                []
            }
            func plan(reference: String) async throws -> InstallPlan {
                makePlan()
            }
            func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
                AsyncThrowingStream { $0.finish() }
            }
        }
        let service = InstallService(providers: [DownProvider()], freeDiskBytes: { _ in .max })
        await #expect(throws: InstallError.providerUnavailable(hint: "Down isn't installed.")) {
            _ = try await service.plan(
                provider: InstallProviderID(rawValue: "down"), reference: "x")
        }
        await #expect(throws: InstallError.providerUnavailable(hint: "Down isn't installed.")) {
            _ = try await service.search(
                provider: InstallProviderID(rawValue: "down"), matching: "x")
        }
    }

    @Test func unknownInstallIDFinishesImmediately() async throws {
        let service = InstallService(providers: [], freeDiskBytes: { _ in .max })
        let events = await collect(await service.events(id: "in-missing"))
        #expect(events.isEmpty)
    }
}
