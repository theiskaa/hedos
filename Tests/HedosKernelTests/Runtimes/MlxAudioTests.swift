import Foundation
import Testing

@testable import HedosKernel

private func mlxAudioRecord() -> ModelRecord {
    ModelRecord(
        name: "Kokoro-82M",
        modality: .speech,
        capabilities: [.speak],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: "~/models/huggingface/hub/models--mlx-community--Kokoro-82M-4bit",
            repo: "mlx-community/Kokoro-82M-4bit"),
        runtime: RuntimeRef(
            id: "python:mlx-audio",
            resolved: .auto,
            tier: .managed),
        execution: .stream,
        footprintMB: 350,
        state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private func fakeSidecarScript() -> URL {
    URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
}

@Test func mlxAudioSpeaksThroughFakeSidecarWithStats() async throws {
    let spec = SidecarSpec(
        runtimeID: "fake-mlxaudio-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScript().path, "normal"],
        readyTimeout: .seconds(15))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let payload: JSONValue = .object(["voice": .string("bella")])
    var control: [String: JSONValue] = ["op": .string("speak")]
    if case .object(let fields) = payload {
        for (key, value) in fields { control[key] = value }
    }

    var audio: [Data] = []
    var statuses: [String] = []
    var stats: GenerationStats?
    let stream = await supervisor.request(spec, .object(control))
    for try await chunk in stream {
        switch chunk {
        case .audio(let frame): audio.append(frame.data)
        case .status(let message): statuses.append(message)
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(audio.count == 3)
    #expect(statuses.contains("generating"))
    #expect(statuses.contains("voice=bella speed=None"))
    #expect(stats?.durationMs == 120)
    await supervisor.shutdownAll()
}

@Test func mlxAudioSpeakForwardsVoiceForLanguageSelection() async throws {
    let spec = SidecarSpec(
        runtimeID: "fake-mlxaudio-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScript().path, "normal"],
        readyTimeout: .seconds(15))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let control: JSONValue = .object([
        "op": .string("speak"), "voice": .string("jf_alpha"), "speed": .double(1.5),
    ])
    var statuses: [String] = []
    for try await chunk in await supervisor.request(spec, control) {
        if case .status(let message) = chunk { statuses.append(message) }
    }
    #expect(statuses.contains("voice=jf_alpha speed=1.5"))
    await supervisor.shutdownAll()
}

@Test func mlxAudioBundleSpeaksEachVoicesLanguage() throws {
    let bundle = try #require(RuntimeBundle.directory(named: "python-mlx-audio"))
    let mainPy = try String(
        contentsOf: bundle.appendingPathComponent("main.py"), encoding: .utf8)
    #expect(!mainPy.contains(#"lang_code="a""#))
    #expect(mainPy.contains("pipeline_for"))
    #expect(mainPy.contains("KOKORO_LANGUAGES"))
    let requirements = try String(
        contentsOf: bundle.appendingPathComponent("requirements.in"), encoding: .utf8)
    #expect(requirements.contains("misaki[en,ja,zh]"))
}

@Test func mlxAudioSpecDeclaresCooperativeCancel() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let descriptor = MlxAudioAdapter.descriptor(environments: .shared, workdirRoot: dir)
    let spec = try descriptor.makeSpec(mlxAudioRecord(), dir.appendingPathComponent("env"))
    #expect(spec.cooperativeCancel == true)
}

@Test func cancelledSpeakKeepsSidecarWarmAndNextUtteranceIsServed() async throws {
    let spec = SidecarSpec(
        runtimeID: "fake-mlxaudio-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScript().path, "slow-speak"],
        readyTimeout: .seconds(15),
        cooperativeCancel: true,
        cancelGraceTimeout: .seconds(10))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.request(
            spec, .object(["op": .string("speak"), "voice": .string("af_heart")]))
        for try await chunk in stream {
            if case .audio = chunk { break }
        }
    }
    _ = await consumer.result

    var settled = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        settled = await supervisor.isRunning(spec.runtimeID)
        if !settled { break }
    }
    #expect(settled)

    var audioFrames = 0
    let second = await supervisor.request(
        spec, .object(["op": .string("speak"), "voice": .string("af_heart")]))
    for try await chunk in second {
        if case .audio = chunk { audioFrames += 1 }
    }
    #expect(audioFrames == 20)
    await supervisor.shutdownAll()
}

@Test func mlxAudioLoadFailureReleasesGateAndLeavesModelUnresident() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 65536)
    let supervisor = SidecarSupervisor()
    let record = mlxAudioRecord()
    let producer = GPUProducer.generation(modelID: record.id)

    let failingSpec = SidecarSpec(
        runtimeID: "fake-mlxaudio-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScript().path, "never-ready"],
        readyTimeout: .milliseconds(200))

    await #expect(throws: KernelError.self) {
        try await SidecarWarmLoad.acquire(
            governor: governor, supervisor: supervisor, spec: failingSpec, record: record,
            producer: producer, startingStatus: "Starting speech runtime…"
        ) { _ in }
    }

    #expect(await governor.isResident(record.id) == false)

    let workingSpec = SidecarSpec(
        runtimeID: "fake-mlxaudio-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScript().path, "normal"],
        readyTimeout: .seconds(15))

    try await SidecarWarmLoad.acquire(
        governor: governor, supervisor: supervisor, spec: workingSpec, record: record,
        producer: producer, startingStatus: "Starting speech runtime…"
    ) { _ in }

    #expect(await supervisor.isRunning(workingSpec.runtimeID))
    await governor.gate.release(producer)
    await supervisor.shutdownAll()
}
