import Foundation
import Testing

@testable import HedosKernel

func fakeSidecarDescriptor(
    spec: SidecarSpec, warmWindow: Duration? = nil,
    prepareEnvironment: @escaping @Sendable (@escaping @Sendable (String) -> Void) async throws ->
        URL? = { _ in nil }
) -> PythonSidecarRuntime.Descriptor {
    PythonSidecarRuntime.Descriptor(
        runtimeID: spec.runtimeID,
        preparingStatus: "Preparing fake runtime…",
        startingStatus: "Starting fake runtime…",
        warmWindow: warmWindow,
        prepareEnvironment: prepareEnvironment,
        makeSpec: { _, _ in spec })
}

private func fakeSpec(mode: String = "normal", cooperativeCancel: Bool = false) -> SidecarSpec {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
    return SidecarSpec(
        runtimeID: "fake-runtime-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", script.path, mode],
        readyTimeout: .seconds(15),
        cooperativeCancel: cooperativeCancel)
}

private func makeRuntime(
    spec: SidecarSpec, supervisor: SidecarSupervisor, governor: MemoryGovernor,
    warmWindow: Duration? = nil
) -> PythonSidecarRuntime {
    PythonSidecarRuntime(
        descriptor: fakeSidecarDescriptor(spec: spec, warmWindow: warmWindow),
        governor: governor, supervisor: supervisor)
}

@Test func streamDeliversTextAndStatsThroughTheLifecycle() async throws {
    let spec = fakeSpec()
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = makeRuntime(spec: spec, supervisor: supervisor, governor: governor)
    let record = Fixtures.gguf()

    var text = ""
    var stats: GenerationStats?
    let stream = runtime.stream(
        record, op: .chat,
        payload: .object([
            "messages": .array([
                .object(["role": .string("user"), "content": .string("hello there world")])
            ])
        ]))
    for try await chunk in stream {
        if case .text(let delta) = chunk { text += delta }
        if case .done(let final) = chunk { stats = final }
    }
    #expect(!text.isEmpty)
    #expect(stats?.promptTokens != nil)
    #expect(await governor.isResident(record.id))
    await supervisor.shutdownAll()
}

@Test func jobReleasesGateOnErrorAndServesTheNextRequest() async throws {
    let spec = fakeSpec()
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = makeRuntime(spec: spec, supervisor: supervisor, governor: governor)
    let record = Fixtures.flux()

    do {
        for try await _ in runtime.job(
            record, op: "image", payload: .object(["prompt": .string("fail")]))
        {}
        Issue.record("the failing job must surface its error")
    } catch {}

    var sawResult = false
    for try await event in runtime.job(
        record, op: "image",
        payload: .object(["prompt": .string("ok"), "steps": .int(2), "seed": .int(5)]))
    {
        if case .result = event { sawResult = true }
    }
    #expect(sawResult)
    await supervisor.shutdownAll()
}

@Test func secondStreamReusesTheWarmSidecarWithoutRespawn() async throws {
    let spec = fakeSpec()
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = makeRuntime(spec: spec, supervisor: supervisor, governor: governor)
    let record = Fixtures.gguf()
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("ping pong")])
        ])
    ])

    for try await _ in runtime.stream(record, op: .chat, payload: payload) {}
    let firstPID = await supervisor.processIdentifier(spec.runtimeID)
    #expect(firstPID != nil)

    for try await _ in runtime.stream(record, op: .chat, payload: payload) {}
    let secondPID = await supervisor.processIdentifier(spec.runtimeID)
    #expect(secondPID == firstPID)
    await supervisor.shutdownAll()
}

@Test func environmentPrepareFailureSurfacesBeforeAnySidecarSpawns() async throws {
    let spec = fakeSpec()
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = PythonSidecarRuntime(
        descriptor: fakeSidecarDescriptor(
            spec: spec,
            prepareEnvironment: { _ in
                throw KernelError.runtimeFailed("uv exploded")
            }),
        governor: governor, supervisor: supervisor)
    let record = Fixtures.gguf()

    do {
        for try await _ in runtime.stream(record, op: .chat, payload: .object([:])) {}
        Issue.record("the prepare failure must surface")
    } catch {}
    #expect(await !supervisor.isRunning(spec.runtimeID))
    #expect(await !governor.isResident(record.id))
    await supervisor.shutdownAll()
}

@Test func cancellingAStreamEndsItAndTheNextRequestStillWorks() async throws {
    let spec = fakeSpec(cooperativeCancel: true)
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = makeRuntime(spec: spec, supervisor: supervisor, governor: governor)
    let record = Fixtures.gguf()
    let slow: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("slow")])
        ])
    ])

    let reader = Task {
        var deltas = 0
        for try await chunk in runtime.stream(record, op: .chat, payload: slow) {
            if case .text = chunk {
                deltas += 1
                if deltas == 2 { break }
            }
        }
        return deltas
    }
    _ = try await reader.value

    var text = ""
    let follow: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hello again friend")])
        ])
    ])
    for try await chunk in runtime.stream(record, op: .chat, payload: follow) {
        if case .text(let delta) = chunk { text += delta }
    }
    #expect(!text.isEmpty)
    await supervisor.shutdownAll()
}
