import Foundation
import Testing

@testable import HedosKernel

@Test func oneShotHoldsTheGateWhileTheBodyRunsAndReleasesAfter() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    var record = Fixtures.gguf()
    record.state = .ready
    let (bodyStarted, startedContinuation) = AsyncStream.makeStream(of: Void.self)
    let (release, releaseContinuation) = AsyncStream.makeStream(of: Void.self)

    let run = Task {
        try await GovernedOneShot.run(
            governor: governor, record: record,
            producer: GPUProducer.job(modelID: record.id),
            status: { _ in }
        ) {
            startedContinuation.yield(())
            startedContinuation.finish()
            var gate = release.makeAsyncIterator()
            _ = await gate.next()
            return 42
        }
    }

    var started = bodyStarted.makeAsyncIterator()
    _ = await started.next()

    let contenderDone = CleanupFlag()
    let contender = Task {
        let producer = GPUProducer.load(modelID: "contender")
        try? await governor.gate.acquire(producer)
        contenderDone.mark()
        await governor.gate.release(producer)
    }

    try await Task.sleep(for: .milliseconds(150))
    #expect(!contenderDone.wasInvoked)

    releaseContinuation.yield(())
    releaseContinuation.finish()
    #expect(try await run.value == 42)

    var freed = false
    for _ in 0..<100 {
        if contenderDone.wasInvoked {
            freed = true
            break
        }
        try await Task.sleep(for: .milliseconds(20))
    }
    #expect(freed)
    _ = await contender.value
}

@Test func oneShotReleasesLeaseWhenCancelledWaitingForGate() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    var record = Fixtures.gguf()
    record.state = .ready
    let (held, holdCont) = AsyncStream.makeStream(of: Void.self)
    let (release, releaseCont) = AsyncStream.makeStream(of: Void.self)

    let holder = Task {
        try await GovernedOneShot.run(
            governor: governor, record: record,
            producer: GPUProducer.job(modelID: record.id), status: { _ in }
        ) {
            holdCont.yield(())
            holdCont.finish()
            var iterator = release.makeAsyncIterator()
            _ = await iterator.next()
            return 0
        }
    }
    var heldIterator = held.makeAsyncIterator()
    _ = await heldIterator.next()

    let recordB: ModelRecord = {
        var record = Fixtures.gguf(path: "/tmp/hedos-tests/b.gguf")
        record.state = .ready
        return record
    }()
    let waiter = Task {
        let _: Int = try await GovernedOneShot.run(
            governor: governor, record: recordB,
            producer: GPUProducer.job(modelID: recordB.id), status: { _ in }
        ) { 99 }
    }
    try await Task.sleep(for: .milliseconds(120))
    waiter.cancel()
    var waiterThrew = false
    do { _ = try await waiter.value } catch { waiterThrew = true }
    #expect(waiterThrew)

    releaseCont.yield(())
    releaseCont.finish()
    _ = try await holder.value

    let probe = GPUProducer.generation(modelID: recordB.id)
    try await governor.gate.acquire(probe)
    await governor.gate.release(probe)
}

@Test func oneShotReleasesTheGateWhenTheBodyThrows() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    var record = Fixtures.gguf()
    record.state = .ready

    await #expect(throws: KernelError.self) {
        let _: Int = try await GovernedOneShot.run(
            governor: governor, record: record,
            producer: GPUProducer.generation(modelID: record.id),
            status: { _ in }
        ) {
            throw KernelError.runtimeFailed("the tool exploded")
        }
    }

    let producer = GPUProducer.load(modelID: "after-failure")
    try? await governor.gate.acquire(producer)
    await governor.gate.release(producer)
}
