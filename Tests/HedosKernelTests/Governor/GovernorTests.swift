import Foundation
import Testing

@testable import HedosKernel

private func testGovernor(
    totalMemoryMB: Int = 65536,
    defaultWarmWindow: Duration = .seconds(300),
    clock: any Clock<Duration> = ContinuousClock()
) -> MemoryGovernor {
    MemoryGovernor(
        totalMemoryMB: totalMemoryMB,
        heavyThresholdMB: 1024,
        defaultWarmWindow: defaultWarmWindow,
        clock: clock)
}

private actor ReasonLog {
    private(set) var reasons: [String] = []

    func append(_ reason: String) {
        reasons.append(reason)
    }
}

private actor UnloadCounter {
    private(set) var count = 0

    func increment() {
        count += 1
    }
}

private struct GatedFakeRuntime: Sendable {
    let gate: GPUGate
    let probe: ConcurrencyProbe
    let producer: GPUProducer

    func run(passes: Int) async {
        for _ in 0..<passes {
            await gate.withAccess(producer) {
                await probe.enter()
                try? await Task.sleep(for: .milliseconds(10))
                await probe.exit()
            }
        }
    }
}

private func governedImageRecord() -> ModelRecord {
    var record = Fixtures.flux()
    record.runtime = RuntimeRef(id: "fake:image", resolved: .auto, tier: .managed)
    return record
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

private func settleScheduledWork(_ times: Int = 50) async {
    for _ in 0..<times {
        await Task.yield()
    }
}

@Test func heavyLoadEvictsIdleResidentThenAdmits() async throws {
    let governor = testGovernor()
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "qwen3.5-9b", footprintMB: 6000) {
        unloaded.mark()
    }

    let verdict = try await governor.admit(
        modelID: "flux", name: "FLUX.1-schnell", footprintMB: 34000)

    #expect(verdict == .ok)
    #expect(unloaded.wasInvoked)
    #expect(await governor.isResident("llm") == false)
}

@Test func heavyLoadWaitsVisiblyWhileLeaseHeldAndAdmitsAfterRelease() async throws {
    let governor = testGovernor()
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "qwen3.5-9b", footprintMB: 6000) {
        unloaded.mark()
    }
    await governor.beginGeneration("llm")

    let reasons = ReasonLog()
    let admitTask = Task {
        try await governor.admit(
            modelID: "flux", name: "FLUX.1-schnell", footprintMB: 34000
        ) { reason in
            await reasons.append(reason)
        }
    }

    try await waitUntil { await !reasons.reasons.isEmpty }
    #expect(await reasons.reasons == ["Waiting for qwen3.5-9b to finish"])
    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("llm"))

    await governor.endGeneration("llm")
    let verdict = try await admitTask.value
    #expect(verdict == .ok)
    #expect(unloaded.wasInvoked)
    #expect(await governor.isResident("llm") == false)
}

@Test func lightLoadCoexistsWithHeavyResident() async throws {
    let governor = testGovernor()
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "flux", name: "FLUX.1-schnell", footprintMB: 34000) {
        unloaded.mark()
    }

    try await governor.admit(modelID: "tts", name: "kokoro", footprintMB: 350)

    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("flux"))
}

@Test func gateSharesSameModelGenerations() async throws {
    let gate = GPUGate()
    let probe = ConcurrencyProbe()

    await withTaskGroup(of: Void.self) { group in
        for _ in 0..<2 {
            group.addTask {
                await gate.withAccess(.generation(modelID: "a")) {
                    await probe.enter()
                    try? await Task.sleep(for: .milliseconds(60))
                    await probe.exit()
                }
            }
        }
    }

    #expect(await probe.peak == 2)
}

@Test func gateSerializesDistinctProducers() async throws {
    let gate = GPUGate()
    let probe = ConcurrencyProbe()
    let producers: [GPUProducer] = [
        .generation(modelID: "a"),
        .generation(modelID: "b"),
        .load(modelID: "c"),
        .unload(modelID: "a"),
        .job(modelID: "d"),
    ]

    await withTaskGroup(of: Void.self) { group in
        for producer in producers {
            group.addTask {
                await gate.withAccess(producer) {
                    await probe.enter()
                    try? await Task.sleep(for: .milliseconds(20))
                    await probe.exit()
                }
            }
        }
    }

    #expect(await probe.peak == 1)
}

@Test func concurrentStreamAndJobNeverInterleaveMetalProducers() async throws {
    let governor = testGovernor()
    let probe = ConcurrencyProbe()
    let stream = GatedFakeRuntime(
        gate: governor.gate, probe: probe, producer: .generation(modelID: "llm"))
    let job = GatedFakeRuntime(
        gate: governor.gate, probe: probe, producer: .job(modelID: "flux"))

    async let streaming: Void = stream.run(passes: 6)
    async let jobbing: Void = job.run(passes: 6)
    _ = await (streaming, jobbing)

    #expect(await probe.peak == 1)
}

@Test func jobNeedingMemoryQueuesVisiblyAndAdmitsAfterLeaseRelease() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let governor = testGovernor()
    let kernel = Kernel(
        directory: dir,
        adapters: [FakeImageAdapter(stepDelay: .milliseconds(5))],
        governor: governor)
    let record = governedImageRecord()
    try await kernel.registry.register(record)

    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "resident-llm", footprintMB: 6000) {
        unloaded.mark()
    }
    await governor.beginGeneration("llm")

    let jobID = try await kernel.submit(record.id, .image, payload: .null)
    try await waitUntil {
        try await kernel.job(id: jobID)?.queueReason != nil
    }
    let waiting = try #require(try await kernel.job(id: jobID))
    #expect(waiting.state == .queued)
    #expect(waiting.queueReason == "Waiting for resident-llm to finish")
    #expect(unloaded.wasInvoked == false)

    await governor.endGeneration("llm")
    for await _ in await kernel.jobEvents(id: jobID) {}

    #expect(try await kernel.job(id: jobID)?.state == .done)
    #expect(unloaded.wasInvoked)
    #expect(await governor.isResident("llm") == false)
}

@Test func unloadAbortsWhileFreshLeaseHeldThenRunsAfterRelease() async throws {
    let governor = testGovernor(defaultWarmWindow: .milliseconds(60))
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 500) {
        unloaded.mark()
    }
    await governor.beginGeneration("llm")
    await governor.residency.unloadNow("llm")

    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("llm"))

    await governor.endGeneration("llm")
    try await waitUntil { await governor.isResident("llm") == false }
    #expect(unloaded.wasInvoked)
}

@Test func secondHeavyAdmitDuringLoadWindowWaitsForReservation() async throws {
    let governor = testGovernor()
    await governor.beginGeneration("h1")
    try await governor.admit(modelID: "h1", name: "heavy-one", footprintMB: 30000)
    #expect(await governor.isResident("h1"))

    let reasons = ReasonLog()
    let second = Task {
        try await governor.admit(
            modelID: "h2", name: "heavy-two", footprintMB: 30000
        ) { reason in
            await reasons.append(reason)
        }
    }
    try await waitUntil { await !reasons.reasons.isEmpty }
    #expect(await reasons.reasons.first == "Waiting for heavy-one to finish")
    #expect(await governor.isResident("h2") == false)

    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "h1", name: "heavy-one", footprintMB: 30000) {
        unloaded.mark()
    }
    await governor.endGeneration("h1")
    _ = try await second.value

    #expect(unloaded.wasInvoked)
    #expect(await governor.isResident("h1") == false)
    #expect(await governor.isResident("h2"))
}

@Test func verdictIsAdvisoryAndAdmitNeverRefuses() async throws {
    let governor = testGovernor(totalMemoryMB: 10000)
    #expect(await governor.verdict(admitting: 7000) == .ok)

    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 4000) {}
    #expect(await governor.verdict(admitting: 7000) == .tight)

    let verdict = try await governor.admit(modelID: "flux", name: "flux", footprintMB: 34000)
    #expect(verdict == .tight)
    #expect(await governor.isResident("llm") == false)
}

@Test func observedFootprintRefinesEstimateAndVerdict() async throws {
    let governor = testGovernor(totalMemoryMB: 10000)
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 4000) {}
    #expect(await governor.verdict(admitting: 5000) == .tight)

    await governor.observeFootprint("llm", footprintMB: 1500)

    #expect(await governor.verdict(admitting: 5000) == .ok)
    #expect(await governor.resident().first?.footprintMB == 1500)
}

@Test func residencyUnloadsAfterWarmWindowOnceLastLeaseDrops() async throws {
    let governor = testGovernor(defaultWarmWindow: .milliseconds(80))
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "tts", name: "kokoro", footprintMB: 350) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")

    try await waitUntil { await governor.isResident("tts") == false }
    #expect(unloaded.wasInvoked)
}

@Test func newGenerationCancelsPendingIdleUnload() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .milliseconds(120), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "tts", name: "kokoro", footprintMB: 350) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")
    await settleScheduledWork()
    clock.advance(by: .milliseconds(40))
    await governor.beginGeneration("tts")
    clock.advance(by: .milliseconds(250))

    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("tts"))

    await governor.endGeneration("tts")
    await settleScheduledWork()
    clock.advance(by: .milliseconds(121))
    try await waitUntil { await governor.isResident("tts") == false }
    #expect(unloaded.wasInvoked)
}

@Test func idleUnloadFiresRealTimeSmoke() async throws {
    let governor = testGovernor(defaultWarmWindow: .milliseconds(60))
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "tts", name: "kokoro", footprintMB: 350) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")

    try await waitUntil { await governor.isResident("tts") == false }
    #expect(unloaded.wasInvoked)
}

@Test func perModelWarmWindowOverridesDefault() async throws {
    let governor = testGovernor(defaultWarmWindow: .seconds(300))
    let unloaded = CleanupFlag()
    await governor.markLoaded(
        modelID: "tts", name: "kokoro", footprintMB: 350, warmWindow: .milliseconds(60)
    ) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")

    try await waitUntil { await governor.isResident("tts") == false }
    #expect(unloaded.wasInvoked)
}

@Test func unloadDrainWaitsForZeroLeases() async throws {
    let lease = ModelLease()
    await lease.acquire("m")
    await lease.acquire("m")
    let drained = CleanupFlag()
    let drainTask = Task {
        try await lease.drain("m")
        drained.mark()
    }

    try await Task.sleep(for: .milliseconds(30))
    #expect(drained.wasInvoked == false)

    await lease.release("m")
    try await Task.sleep(for: .milliseconds(30))
    #expect(drained.wasInvoked == false)

    await lease.release("m")
    try await drainTask.value
    #expect(drained.wasInvoked)
    #expect(await lease.count("m") == 0)
}

@Test func concurrentHeavyAdmitsSerializeUnderStrictSingle() async throws {
    let governor = testGovernor()
    let oldUnloaded = CleanupFlag()
    let unloadCounter = UnloadCounter()
    await governor.markLoaded(modelID: "old", name: "old-heavy", footprintMB: 6000) {
        oldUnloaded.mark()
        await unloadCounter.increment()
    }

    async let a: RAMVerdict = governor.admit(modelID: "a", name: "heavy-a", footprintMB: 30000)
    async let b: RAMVerdict = governor.admit(modelID: "b", name: "heavy-b", footprintMB: 30000)
    _ = try await (a, b)

    let heavyResidents = await governor.resident().filter { $0.footprintMB >= 1024 }
    #expect(heavyResidents.count == 1)
    #expect(oldUnloaded.wasInvoked)
    #expect(await unloadCounter.count == 1)
}

@Test func admissionChainSurvivesCancellation() async throws {
    let governor = testGovernor()
    let oldUnloaded = CleanupFlag()
    await governor.markLoaded(modelID: "old", name: "old-heavy", footprintMB: 6000) {
        oldUnloaded.mark()
    }
    await governor.beginGeneration("old")

    let aTask = Task {
        try await governor.admit(modelID: "a", name: "heavy-a", footprintMB: 30000)
    }
    try await waitUntil { await governor.leases.count("old") > 0 }
    aTask.cancel()
    let aResult = await aTask.result
    #expect(throws: (any Error).self) { try aResult.get() }

    await governor.endGeneration("old")

    let bResult = try await governor.admit(modelID: "b", name: "heavy-b", footprintMB: 30000)
    #expect(bResult == .ok)
    #expect(await governor.isResident("b"))
}

@Test func footprintPromotionEvictsCoResidentHeavy() async throws {
    let governor = testGovernor()
    let bUnloaded = CleanupFlag()
    await governor.markLoaded(modelID: "B", name: "heavy-b", footprintMB: 2048) {
        bUnloaded.mark()
    }
    _ = try await governor.admit(modelID: "A", name: "under-a", footprintMB: 512)
    await governor.markLoaded(modelID: "A", name: "under-a", footprintMB: 512) {}
    #expect(await governor.isResident("B"))
    #expect(await governor.isResident("A"))

    await governor.observeFootprint("A", footprintMB: 2048)
    try await waitUntil { bUnloaded.wasInvoked }
    #expect(await governor.isResident("B") == false)
}

@Test func budgetedEvictionEvictsSmallestOldestToMakeRoom() async throws {
    let governor = testGovernor(totalMemoryMB: 65536)
    await governor.apply(
        policy: ResidencyPolicy(keepWarm: .fiveMinutes, eviction: .budgeted, ramBudgetMB: 3000))
    for id in ["s1", "s2", "s3"] {
        _ = try await governor.admit(modelID: id, name: id, footprintMB: 500)
        await governor.markLoaded(modelID: id, name: id, footprintMB: 500) {}
    }
    _ = try await governor.admit(modelID: "heavy", name: "heavy", footprintMB: 2000)
    await governor.markLoaded(modelID: "heavy", name: "heavy", footprintMB: 2000) {}

    #expect(await governor.isResident("s1") == false)
    #expect(await governor.isResident("s2"))
    #expect(await governor.isResident("s3"))
    #expect(await governor.isResident("heavy"))
    let occupied = await governor.resident().reduce(0) { $0 + $1.footprintMB }
    #expect(occupied <= 3000)
}

@Test func tighteningWarmWindowReschedulesLiveIdleTimer() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .seconds(3600), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "tts", name: "kokoro", footprintMB: 350) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")
    await settleScheduledWork()

    await governor.setWarmWindow(.milliseconds(50), for: "tts")
    await settleScheduledWork()
    clock.advance(by: .milliseconds(60))
    try await waitUntil { await governor.isResident("tts") == false }
    #expect(unloaded.wasInvoked)
}

@Test func lengtheningWarmWindowDoesNotFireAtOldDeadline() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .milliseconds(50), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "tts", name: "kokoro", footprintMB: 350) {
        unloaded.mark()
    }
    await governor.beginGeneration("tts")
    await governor.endGeneration("tts")
    await settleScheduledWork()

    await governor.setWarmWindow(.seconds(3600), for: "tts")
    await settleScheduledWork()
    clock.advance(by: .milliseconds(200))
    await settleScheduledWork()
    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("tts"))
}

@Test func inPlaceSwapDeregistersSoNoStaleTimerFires() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .milliseconds(50), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "m", name: "m", footprintMB: 500) {
        unloaded.mark()
    }
    await governor.beginGeneration("m")
    await governor.endGeneration("m")
    await settleScheduledWork()

    await governor.markUnloaded("m")
    clock.advance(by: .milliseconds(200))
    await settleScheduledWork()
    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("m") == false)
}

@Test func gateAcquireThrowsWhenCancelledWhileQueued() async throws {
    let gate = GPUGate()
    try await gate.acquire(.load(modelID: "a"))

    let threw = CleanupFlag()
    let queued = Task {
        do {
            try await gate.acquire(.load(modelID: "b"))
            await gate.release(.load(modelID: "b"))
        } catch {
            threw.mark()
        }
    }
    try await Task.sleep(for: .milliseconds(30))
    queued.cancel()
    _ = await queued.value
    #expect(threw.wasInvoked)

    await gate.release(.load(modelID: "a"))
    try await gate.acquire(.load(modelID: "c"))
    await gate.release(.load(modelID: "c"))
}

@Test func lateGenerationAfterQuitSuspensionNeverReArmsUnload() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .milliseconds(50), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 6000) {
        unloaded.mark()
    }
    await governor.suspendForQuit()
    await governor.beginGeneration("llm")
    await governor.endGeneration("llm")
    await settleScheduledWork()
    clock.advance(by: .milliseconds(200))
    await settleScheduledWork()
    #expect(unloaded.wasInvoked == false)
}

@Test func quitTeardownSkipsUnloadsEntirely() async throws {
    let clock = TestClock()
    let governor = testGovernor(defaultWarmWindow: .milliseconds(50), clock: clock)
    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 6000) {
        unloaded.mark()
    }
    await governor.beginGeneration("llm")
    await governor.endGeneration("llm")
    await governor.suspendForQuit()
    await settleScheduledWork()

    clock.advance(by: .milliseconds(200))
    await settleScheduledWork()
    #expect(unloaded.wasInvoked == false)
    #expect(await governor.isResident("llm"))
}
