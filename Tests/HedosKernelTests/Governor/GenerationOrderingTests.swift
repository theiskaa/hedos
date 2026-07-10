import Foundation
import Testing

@testable import HedosKernel

private func orderingGovernor() -> MemoryGovernor {
    MemoryGovernor(
        totalMemoryMB: 65536,
        heavyThresholdMB: 1024,
        defaultWarmWindow: .seconds(300))
}

private func waitUntil(
    _ condition: @Sendable () async throws -> Bool
) async throws {
    for _ in 0..<800 {
        if try await condition() { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("condition never became true")
}

private actor SignalGate {
    private var opened = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func wait() async {
        if opened { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    func open() {
        opened = true
        let pending = waiters
        waiters.removeAll()
        for waiter in pending {
            waiter.resume()
        }
    }
}

private actor ChoreographyEngine {
    private let governor: MemoryGovernor
    private let slot = GenerationSlot()
    private var loadedModelID: String?

    init(governor: MemoryGovernor) {
        self.governor = governor
    }

    func run(
        modelID: String,
        name: String,
        footprintMB: Int,
        onWait: (@Sendable (String) async -> Void)? = nil,
        generate: @escaping @Sendable () async -> Void
    ) async throws {
        await governor.beginGeneration(modelID)
        do {
            let producer = GPUProducer.generation(modelID: modelID)
            try await acquireGateWithModelLoaded(
                producer: producer, modelID: modelID, name: name,
                footprintMB: footprintMB, onWait: onWait)
            await slot.acquire()
            await generate()
            await slot.release()
            await governor.gate.release(producer)
        } catch {
            await governor.endGeneration(modelID)
            throw error
        }
        await governor.endGeneration(modelID)
    }

    private func acquireGateWithModelLoaded(
        producer: GPUProducer,
        modelID: String,
        name: String,
        footprintMB: Int,
        onWait: (@Sendable (String) async -> Void)?
    ) async throws {
        while true {
            try await ensureLoadedGoverned(
                modelID: modelID, name: name, footprintMB: footprintMB, onWait: onWait)
            await governor.gate.acquire(producer)
            if loadedModelID == modelID { return }
            await governor.gate.release(producer)
        }
    }

    private func ensureLoadedGoverned(
        modelID: String,
        name: String,
        footprintMB: Int,
        onWait: (@Sendable (String) async -> Void)?
    ) async throws {
        if loadedModelID == modelID { return }
        try await governor.admit(
            modelID: modelID, name: name, footprintMB: footprintMB, onWait: onWait)
        let load = GPUProducer.load(modelID: modelID)
        await governor.gate.acquire(load)
        if let previous = loadedModelID {
            loadedModelID = nil
            await governor.markUnloaded(previous)
        }
        loadedModelID = modelID
        await governor.gate.release(load)
        await governor.markLoaded(
            modelID: modelID, name: name, footprintMB: footprintMB
        ) { [weak self] in
            await self?.unloadIfLoaded(modelID)
        }
    }

    private func unloadIfLoaded(_ modelID: String) {
        if loadedModelID == modelID {
            loadedModelID = nil
        }
    }
}

@Test func twoOverlappingGenerationsOnOneEngineFamilyComplete() async throws {
    let governor = orderingGovernor()
    let engine = ChoreographyEngine(governor: governor)
    try await engine.run(modelID: "y", name: "heavy-y", footprintMB: 30000) {}

    let holdY = SignalGate()
    let yGenerating = CleanupFlag()
    let yDone = CleanupFlag()
    let xDone = CleanupFlag()
    let waitingSeen = CleanupFlag()

    let generationB = Task {
        try await engine.run(modelID: "y", name: "heavy-y", footprintMB: 30000) {
            yGenerating.mark()
            await holdY.wait()
        }
        yDone.mark()
    }
    try await waitUntil { yGenerating.wasInvoked }

    let generationA = Task {
        try await engine.run(
            modelID: "x", name: "heavy-x", footprintMB: 30000,
            onWait: { _ in waitingSeen.mark() }
        ) {}
        xDone.mark()
    }

    try await waitUntil { waitingSeen.wasInvoked }
    await holdY.open()
    try await waitUntil { yDone.wasInvoked && xDone.wasInvoked }

    #expect(yDone.wasInvoked)
    #expect(xDone.wasInvoked)
    #expect(await governor.isResident("x"))
    #expect(await governor.isResident("y") == false)

    generationA.cancel()
    generationB.cancel()
}

@Test func sameModelGenerationsSerializeOnSlot() async throws {
    let governor = orderingGovernor()
    let engine = ChoreographyEngine(governor: governor)
    let probe = ConcurrencyProbe()

    try await withThrowingTaskGroup(of: Void.self) { group in
        for _ in 0..<2 {
            group.addTask {
                try await engine.run(modelID: "m", name: "heavy-m", footprintMB: 30000) {
                    await probe.enter()
                    try? await Task.sleep(for: .milliseconds(40))
                    await probe.exit()
                }
            }
        }
        try await group.waitForAll()
    }

    #expect(await probe.peak == 1)
}
