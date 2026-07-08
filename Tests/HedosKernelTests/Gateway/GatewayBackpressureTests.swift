import Foundation
import Testing

@testable import HedosKernel

private func readyChatModel() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/busy-model.gguf")
    record.name = "busy-model"
    record.state = .ready
    record.footprintMB = 8000
    return record
}

private func waitUntil(
    ceiling: Duration = .seconds(5), interval: Duration = .milliseconds(20),
    _ condition: @Sendable () async -> Bool
) async {
    var elapsed: Duration = .zero
    while elapsed < ceiling {
        if await condition() { return }
        try? await Task.sleep(for: interval)
        elapsed += interval
    }
}

@Test func saturatedAdmissionAnswers503WithRetryAfterAndAudits() async throws {
    let port = FakeGatewayPort(
        records: [readyChatModel()],
        chatScript: [.text("never"), .done(nil)],
        admission: .saturated(retryAfterSeconds: 1))
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json([
        "model": "busy-model",
        "messages": [["role": "user", "content": "hi"]],
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 503)
    #expect(http.value(forHTTPHeaderField: "Retry-After") == "1")
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect(error?["type"] as? String == "overloaded")

    let entries = await stack.audit.tail(limit: 5)
    #expect(entries.last?.outcome == "saturated")
    #expect(entries.last?.status == 503)
    await stack.stop()
}

@Test func saturatedJobQueueAnswers503OnImages() async throws {
    var model = Fixtures.flux()
    model.state = .ready
    let port = FakeGatewayPort(
        records: [model],
        jobResult: ["never-used"],
        admission: .saturated(retryAfterSeconds: 5))
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["model": "FLUX.1-schnell", "prompt": "queued out"])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/images/generations"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 503)
    #expect(http.value(forHTTPHeaderField: "Retry-After") == "5")
    await stack.stop()
}

private actor Signal {
    private var fired = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func fire() {
        fired = true
        for waiter in waiters { waiter.resume() }
        waiters = []
    }

    func wait() async {
        if fired { return }
        await withCheckedContinuation { waiters.append($0) }
    }
}

@Test func inflightCapAnswers503WhenExceeded() async throws {
    struct HangingHandler: GatewayHandling {
        let entered: Signal
        let release: Signal
        var surface: GatewaySurface { .openAI }
        func handle(
            _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
            responder: GatewayResponder
        ) async throws -> GatewayOutcome {
            await entered.fire()
            await release.wait()
            try await responder.respond(status: 200, body: Data("{}".utf8))
            return .ok
        }
    }
    let entered = Signal()
    let release = Signal()
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(),
        routes: [
            GatewayRoute(
                "POST", "/v1/chat/completions", HangingHandler(entered: entered, release: release),
                inference: true)
        ],
        maxConcurrentInference: 1)
    let body = GatewayHarness.json(["model": "x"])

    let hanging = Task {
        try? await URLSession.shared.data(
            for: GatewayHarness.request(
                "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    }
    await entered.wait()

    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/chat/completions"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 503)
    #expect(http.value(forHTTPHeaderField: "Retry-After") == "1")

    let entries = await stack.audit.tail(limit: 5)
    #expect(entries.last?.outcome == "saturated")
    await release.fire()
    hanging.cancel()
    await stack.stop()
}

@Test func governorWouldWaitTruthTable() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 32768, heavyThresholdMB: 1024)
    #expect(await governor.wouldWait(admitting: "small", footprintMB: 100) == false)
    #expect(await governor.wouldWait(admitting: "big", footprintMB: 8000) == false)

    await governor.markLoaded(
        modelID: "resident", name: "resident-model", footprintMB: 9000) {}
    #expect(await governor.wouldWait(admitting: "big", footprintMB: 8000) == false)

    await governor.beginGeneration("resident")
    #expect(await governor.wouldWait(admitting: "big", footprintMB: 8000) == true)
    #expect(await governor.wouldWait(admitting: "resident", footprintMB: 9000) == false)
    #expect(await governor.wouldWait(admitting: "small", footprintMB: 100) == false)

    await governor.endGeneration("resident")
    #expect(await governor.wouldWait(admitting: "big", footprintMB: 8000) == false)
}

@Test func schedulerQueueDepthCountsQueuedAndExecuting() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let scheduler = JobScheduler(history: JobHistoryStore(directory: dir))
    #expect(await scheduler.queueDepth() == 0)

    for _ in 0..<3 {
        _ = await scheduler.submit(modelID: "m", capability: .image, payload: .null) {
            AsyncThrowingStream { (continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation) in
                Task {
                    try? await Task.sleep(for: .seconds(2))
                    continuation.yield(.result(data: Data([1]), fileExtension: "png"))
                    continuation.finish()
                }
            }
        }
    }
    await waitUntil { await scheduler.queueDepth() == 3 }
    let depth = await scheduler.queueDepth()
    #expect(depth == 3)
}
