import Foundation
import Synchronization
import Testing

@testable import HedosKernel

final class CleanupFlag: Sendable {
    private let flagged = Mutex(false)

    func mark() {
        flagged.withLock { $0 = true }
    }

    var wasInvoked: Bool {
        flagged.withLock { $0 }
    }
}

actor ConcurrencyProbe {
    private var current = 0
    private(set) var peak = 0

    func enter() {
        current += 1
        peak = max(peak, current)
    }

    func exit() {
        current -= 1
    }
}

actor GateAdmission: JobAdmission {
    private var waiters: [UUID: CheckedContinuation<Void, any Error>] = [:]
    private var open = false

    func admit(_ job: Job, onWait: @escaping @Sendable (String) async -> Void) async throws {
        if open { return }
        await onWait("Waiting for 30 GB of memory")
        let token = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                if open {
                    continuation.resume()
                } else if Task.isCancelled {
                    continuation.resume(throwing: CancellationError())
                } else {
                    waiters[token] = continuation
                }
            }
        } onCancel: {
            Task { await self.abandon(token) }
        }
    }

    func release() {
        open = true
        for waiter in waiters.values {
            waiter.resume()
        }
        waiters = [:]
    }

    private func abandon(_ token: UUID) {
        guard let waiter = waiters.removeValue(forKey: token) else { return }
        waiter.resume(throwing: CancellationError())
    }
}

struct FakeImageAdapter: RuntimeAdapter, JobRunning {
    var totalSteps = 4
    var stepDelay: Duration = .milliseconds(30)
    var cleanup: CleanupFlag? = nil

    var id: String { "fake:image" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(throwing: KernelError.runtimeFailed("fake adapter only runs jobs"))
        }
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        let steps = totalSteps
        let delay = stepDelay
        let cleanup = cleanup
        return AsyncThrowingStream { continuation in
            let task = Task {
                continuation.yield(.status("Preparing fake image runtime"))
                continuation.yield(.started)
                for step in 1...steps {
                    try await Task.sleep(for: delay)
                    continuation.yield(.progress(step: step, totalSteps: steps))
                    if step == 1 {
                        continuation.yield(.preview(Data([0x89, 0x50, 0x4E, 0x47])))
                    }
                }
                continuation.yield(.artifacts(["artifact-\(record.id)"]))
                continuation.finish()
            }
            continuation.onTermination = { reason in
                task.cancel()
                if case .cancelled = reason {
                    cleanup?.mark()
                }
            }
        }
    }
}

struct StreamOnlyAdapter: RuntimeAdapter {
    var id: String { "fake:stream-only" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}

private func fakeImageRecord() -> ModelRecord {
    var record = Fixtures.flux()
    record.runtime = RuntimeRef(id: "fake:image", resolved: .auto, tier: .managed)
    return record
}

private func firstProgressIndex(_ events: [JobEvent]) -> Int? {
    events.firstIndex {
        if case .progress = $0 { return true }
        return false
    }
}

private func progressFractions(_ events: [JobEvent]) -> [Double] {
    events.compactMap {
        if case .progress(let progress) = $0 { return progress.fraction }
        return nil
    }
}

private func waitUntil(
    _ condition: @Sendable () async throws -> Bool
) async throws {
    for _ in 0..<500 {
        if try await condition() { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("condition never became true")
}

@Test func submittedJobStreamsProgressPreviewAndCancelsMidRun() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let cleanup = CleanupFlag()
    let kernel = Kernel(directory: dir, adapters: [FakeImageAdapter(cleanup: cleanup)])
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(
        record.id, .image, payload: .object(["prompt": .string("a lighthouse at dusk")]))

    var events: [JobEvent] = []
    for await event in await kernel.jobEvents(id: jobID) {
        events.append(event)
        if case .progress(let progress) = event, progress.step == 2 {
            await kernel.cancel(jobID: jobID)
        }
    }

    #expect(events.last == .cancelled)
    #expect(events.contains(.running))
    let runningIndex = try #require(events.firstIndex(of: .running))
    let progressIndex = try #require(firstProgressIndex(events))
    #expect(runningIndex < progressIndex)
    let fractions = progressFractions(events)
    #expect(!fractions.isEmpty)
    #expect(fractions == fractions.sorted())
    #expect(
        events.contains {
            if case .preview = $0 { return true }
            return false
        })
    #expect(cleanup.wasInvoked)

    let job = try #require(try await kernel.job(id: jobID))
    #expect(job.state == .cancelled)
    #expect(job.finishedAt != nil)
    #expect(job.result.isEmpty)
}

@Test func jobRunsToDoneAndHistorySurvivesReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [FakeImageAdapter(stepDelay: .milliseconds(5))])
    let record = fakeImageRecord()
    try await kernel.registry.register(record)
    let payload: JSONValue = .object(["prompt": .string("a lighthouse at dusk")])

    let jobID = try await kernel.submit(record.id, .image, payload: payload)

    var events: [JobEvent] = []
    for await event in await kernel.jobEvents(id: jobID) {
        events.append(event)
    }

    #expect(events.last == .done(result: ["artifact-\(record.id)"]))
    let live = try #require(try await kernel.job(id: jobID))
    #expect(live.state == .done)
    #expect(live.progress.fraction == 1)
    #expect(live.result == ["artifact-\(record.id)"])

    let reloaded = JobHistoryStore(directory: dir)
    let listed = try await reloaded.list()
    let recorded = try #require(listed.first { $0.id == jobID })
    #expect(recorded.state == .done)
    #expect(recorded.result == ["artifact-\(record.id)"])
    #expect(recorded.modelID == record.id)
    #expect(recorded.capability == .image)
    #expect(recorded.payload == payload)
    #expect(recorded.error == nil)

    let freshKernel = Kernel(directory: dir, adapters: [])
    let fromHistory = try #require(try await freshKernel.job(id: jobID))
    #expect(fromHistory.state == .done)
    #expect(try await freshKernel.jobHistory().map(\.id).contains(jobID))
}

@Test func jobWaitsInQueuedVisiblyUntilAdmitted() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let admission = GateAdmission()
    let scheduler = JobScheduler(
        history: JobHistoryStore(directory: dir), admission: admission)
    let adapter = FakeImageAdapter(totalSteps: 2, stepDelay: .milliseconds(5))
    let record = fakeImageRecord()

    let jobID = await scheduler.submit(
        modelID: record.id, capability: .image, payload: .null
    ) {
        adapter.run(record, .image, payload: .null)
    }

    try await waitUntil {
        try await scheduler.job(id: jobID)?.queueReason != nil
    }
    let waiting = try #require(try await scheduler.job(id: jobID))
    #expect(waiting.state == .queued)
    #expect(waiting.queueReason == "Waiting for 30 GB of memory")

    let stream = await scheduler.events(id: jobID)
    await admission.release()

    var events: [JobEvent] = []
    for await event in stream {
        events.append(event)
    }

    #expect(events.first == .queued(reason: "Waiting for 30 GB of memory"))
    let queuedIndex = try #require(events.firstIndex(of: .queued(reason: "Waiting for 30 GB of memory")))
    let runningIndex = try #require(events.firstIndex(of: .running))
    #expect(queuedIndex < runningIndex)
    if case .done = try #require(events.last) {} else {
        Issue.record("expected terminal done event, got \(String(describing: events.last))")
    }
    let finished = try #require(try await scheduler.job(id: jobID))
    #expect(finished.state == .done)
    #expect(finished.queueReason == nil)
}

@Test func cancellingJobWaitingForAdmissionUnblocksQueue() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let admission = GateAdmission()
    let scheduler = JobScheduler(
        history: JobHistoryStore(directory: dir), admission: admission)
    let adapter = FakeImageAdapter(totalSteps: 2, stepDelay: .milliseconds(5))
    let record = fakeImageRecord()
    let runner: JobScheduler.Runner = { adapter.run(record, .image, payload: .null) }

    let first = await scheduler.submit(
        modelID: record.id, capability: .image, payload: .null, runner: runner)
    let second = await scheduler.submit(
        modelID: record.id, capability: .image, payload: .null, runner: runner)

    try await waitUntil {
        try await scheduler.job(id: first)?.queueReason != nil
    }
    await scheduler.cancel(first)

    try await waitUntil {
        try await scheduler.job(id: first)?.state == .cancelled
    }
    let cancelled = try #require(try await scheduler.job(id: first))
    #expect(cancelled.startedAt == nil)

    try await waitUntil {
        try await scheduler.job(id: second)?.queueReason != nil
    }
    await admission.release()
    for await _ in await scheduler.events(id: second) {}
    #expect(try await scheduler.job(id: second)?.state == .done)
}

@Test func executionIsSerialOneJobAtATime() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let scheduler = JobScheduler(history: JobHistoryStore(directory: dir))
    let probe = ConcurrencyProbe()

    let runner: JobScheduler.Runner = {
        AsyncThrowingStream { continuation in
            let task = Task {
                await probe.enter()
                continuation.yield(.started)
                try await Task.sleep(for: .milliseconds(40))
                await probe.exit()
                continuation.yield(.artifacts(["artifact"]))
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    let first = await scheduler.submit(
        modelID: "model-a", capability: .image, payload: .null, runner: runner)
    let second = await scheduler.submit(
        modelID: "model-b", capability: .image, payload: .null, runner: runner)

    try await waitUntil {
        try await scheduler.job(id: first)?.state == .running
    }
    #expect(try await scheduler.job(id: second)?.state == .queued)

    for await _ in await scheduler.events(id: second) {}

    #expect(try await scheduler.job(id: first)?.state == .done)
    #expect(try await scheduler.job(id: second)?.state == .done)
    #expect(await probe.peak == 1)
}

@Test func cancellingQueuedJobRemovesItWithoutRunning() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let scheduler = JobScheduler(history: JobHistoryStore(directory: dir))
    let adapter = FakeImageAdapter(totalSteps: 20, stepDelay: .milliseconds(20))
    let record = fakeImageRecord()
    let runner: JobScheduler.Runner = { adapter.run(record, .image, payload: .null) }

    let first = await scheduler.submit(
        modelID: record.id, capability: .image, payload: .null, runner: runner)
    let second = await scheduler.submit(
        modelID: record.id, capability: .image, payload: .null, runner: runner)

    try await waitUntil {
        try await scheduler.job(id: first)?.state == .running
    }
    await scheduler.cancel(second)

    let cancelled = try #require(try await scheduler.job(id: second))
    #expect(cancelled.state == .cancelled)
    #expect(cancelled.startedAt == nil)

    let history = try await JobHistoryStore(directory: dir).get(id: second)
    #expect(history?.state == .cancelled)

    await scheduler.cancel(first)
    for await _ in await scheduler.events(id: first) {}
    #expect(try await scheduler.job(id: first)?.state == .cancelled)
}

@Test func lateSubscriberToFinishedJobGetsTerminalEventAndFinishes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [FakeImageAdapter(stepDelay: .milliseconds(5))])
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(record.id, .image, payload: .null)
    for await _ in await kernel.jobEvents(id: jobID) {}

    var events: [JobEvent] = []
    for await event in await kernel.jobEvents(id: jobID) {
        events.append(event)
    }
    #expect(events == [.done(result: ["artifact-\(record.id)"])])
}

@Test func submitUnknownModelThrowsModelNotFound() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [FakeImageAdapter()])

    await #expect(throws: KernelError.self) {
        try await kernel.submit("missing", .image, payload: .null)
    }
}

@Test func submitOnStreamOnlyAdapterFailsWithoutQueuing() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [StreamOnlyAdapter()])
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    await #expect(throws: KernelError.self) {
        try await kernel.submit(record.id, .image, payload: .null)
    }
    #expect(await kernel.activeJobs().isEmpty)
}

@Test func historyKeepsOnlyMostRecentJobs() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = JobHistoryStore(directory: dir, limit: 3)

    for index in 0..<5 {
        let job = Job(
            id: "job-\(index)",
            modelID: "model",
            capability: .image,
            payload: .null,
            state: .done,
            submittedAt: Date(timeIntervalSince1970: Double(1_750_000_000 + index)))
        try await store.record(job)
    }

    let listed = try await store.list()
    #expect(listed.map(\.id) == ["job-4", "job-3", "job-2"])

    let reloaded = try await JobHistoryStore(directory: dir, limit: 3).list()
    #expect(reloaded.map(\.id) == ["job-4", "job-3", "job-2"])
}

@Test func historyStoreUpsertsByJobID() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = JobHistoryStore(directory: dir)
    var job = Job(
        id: "job-1", modelID: "model", capability: .image, payload: .null,
        state: .failed, error: "first attempt",
        submittedAt: Date(timeIntervalSince1970: 1_750_000_000))
    try await store.record(job)
    job.state = .done
    job.error = nil
    job.result = ["artifact"]
    try await store.record(job)

    let listed = try await store.list()
    #expect(listed.count == 1)
    #expect(listed[0].state == .done)
    #expect(listed[0].result == ["artifact"])
}

@Test func jobHistoryOmitsPreviewFrames() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = JobHistoryStore(directory: dir)
    var job = Job(
        id: "job-1", modelID: "model", capability: .image, payload: .null, state: .done,
        submittedAt: Date(timeIntervalSince1970: 1_750_000_000))
    job.preview = Data([0x89, 0x50])
    try await store.record(job)

    let reloaded = try await JobHistoryStore(directory: dir).get(id: "job-1")
    #expect(reloaded?.preview == nil)
    let raw = try JSONSerialization.jsonObject(
        with: Data(contentsOf: dir.appendingPathComponent("jobs.json"))) as! [String: Any]
    #expect(raw["schemaVersion"] as? Int == 1)
    let jobs = try #require(raw["jobs"] as? [[String: Any]])
    #expect(jobs[0]["preview"] == nil)
}
