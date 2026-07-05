import Foundation
import Testing

@testable import HedosKernel

@Test func frameCodecRoundTripsControlAndAudio() throws {
    let control = Frame.control(.object(["op": .string("speak"), "n": .int(3)]))
    let audio = Frame.audio(Data(repeating: 0x7F, count: 4096))

    var decoder = FrameCodec.Decoder()
    var wire = try FrameCodec.encode(control)
    wire.append(try FrameCodec.encode(audio))
    let frames = try decoder.append(wire)
    #expect(frames == [control, audio])
}

@Test func frameCodecHandlesArbitrarySplits() throws {
    let frame = Frame.audio(Data((0..<1000).map { UInt8($0 % 256) }))
    let wire = try FrameCodec.encode(frame)

    var decoder = FrameCodec.Decoder()
    var collected: [Frame] = []
    for byte in wire {
        collected.append(contentsOf: try decoder.append(Data([byte])))
    }
    #expect(collected == [frame])
}

@Test func frameCodecRejectsOversizedAndUnknown() throws {
    var decoder = FrameCodec.Decoder()
    var oversize = Data()
    var length = UInt32(FrameCodec.maxFrameBytes + 5).littleEndian
    oversize.append(Data(bytes: &length, count: 4))
    #expect(throws: FrameCodecError.self) {
        _ = try decoder.append(oversize)
    }

    var unknown = Data()
    var okLength = UInt32(2).littleEndian
    unknown.append(Data(bytes: &okLength, count: 4))
    unknown.append(contentsOf: [9, 0])
    var second = FrameCodec.Decoder()
    #expect(throws: FrameCodecError.self) {
        _ = try second.append(unknown)
    }
}

@Test func environmentManagerSkipsWhenCurrentMatchesAndRebuildsOnLockChange() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let lockA = root.appendingPathComponent("a.lock")
    let lockB = root.appendingPathComponent("b.lock")
    try Data("packages-a".utf8).write(to: lockA)
    try Data("packages-b".utf8).write(to: lockB)

    final class Counter: @unchecked Sendable {
        let lock = NSLock()
        var value = 0
        func bump() {
            lock.lock()
            value += 1
            lock.unlock()
        }
    }
    let builds = Counter()
    let manager = EnvironmentManager(root: root) { envDir, _, _, progress in
        builds.bump()
        progress("building")
        try FileManager.default.createDirectory(
            at: envDir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    }

    let first = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    let again = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    #expect(first == again)
    #expect(builds.value == 1)

    let updated = try await manager.prepare(runtimeID: "python:test", lockfile: lockB) { _ in }
    #expect(updated != first)
    #expect(builds.value == 2)

    let rolledBack = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    #expect(rolledBack == first)
    #expect(builds.value == 2)

    let current = root.appendingPathComponent("runtimes/python-test/current")
    let destination = try FileManager.default.destinationOfSymbolicLink(atPath: current.path)
    #expect(destination.hasSuffix(try EnvironmentManager.lockHash(lockA)))
}

private func fakeSidecarSpec(mode: String = "normal", idle: Duration = .seconds(120)) -> SidecarSpec
{
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appendingPathComponent("FakeSidecar.py")
    return SidecarSpec(
        runtimeID: "fake-\(mode)-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", script.path, mode],
        readyTimeout: .seconds(15),
        idleTimeout: idle)
}

@Test func supervisorStreamsAudioFromFakeSidecar() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var audio: [Data] = []
    var statuses: [String] = []
    var stats: GenerationStats?
    let stream = await supervisor.request(spec, .object(["op": .string("speak")]))
    for try await chunk in stream {
        switch chunk {
        case .audio(let frame):
            #expect(frame.sampleRate == 16000)
            audio.append(frame.data)
        case .status(let message): statuses.append(message)
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(audio.count == 3)
    #expect(audio[0] == Data(repeating: 0, count: 640))
    #expect(statuses.contains("generating"))
    #expect(stats?.durationMs == 120)
    await supervisor.shutdownAll()
}

@Test func supervisorSurfacesCrashMidRequest() async throws {
    let spec = fakeSidecarSpec(mode: "crash-mid-request")
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let stream = await supervisor.request(spec, .object(["op": .string("speak")]))
    var received = 0
    await #expect(throws: KernelError.self) {
        for try await chunk in stream {
            if case .audio = chunk { received += 1 }
        }
    }
    #expect(received == 1)
    await supervisor.shutdownAll()
}

@Test func supervisorIdleTimeoutShutsSidecarDown() async throws {
    let spec = fakeSidecarSpec(idle: .milliseconds(400))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let stream = await supervisor.request(spec, .object(["op": .string("speak")]))
    for try await _ in stream {}

    try await Task.sleep(for: .seconds(2))
    let respawnStream = await supervisor.request(spec, .object(["op": .string("speak")]))
    var frames = 0
    for try await chunk in respawnStream {
        if case .audio = chunk { frames += 1 }
    }
    #expect(frames == 3)
    await supervisor.shutdownAll()
}

@Test func supervisorTimesOutWhenSidecarNeverReady() async throws {
    var spec = fakeSidecarSpec(mode: "never-ready")
    spec = SidecarSpec(
        runtimeID: spec.runtimeID,
        executable: spec.executable,
        arguments: spec.arguments,
        readyTimeout: .milliseconds(800),
        idleTimeout: spec.idleTimeout)
    let supervisor = SidecarSupervisor()
    await #expect(throws: KernelError.self) {
        try await supervisor.ensureRunning(spec)
    }
    await supervisor.shutdownAll()
}

@Test func mlxAudioAdapterBidMatrix() {
    let adapter = MlxAudioAdapter()
    let speechMlx = IdentifiedModel(
        format: .safetensors, modality: .speech, capabilities: [.speak], execution: .stream)
    let speechUnknown = IdentifiedModel(
        format: .unknown, modality: .speech, capabilities: [.speak], execution: .stream)
    let textGguf = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat], execution: .stream)

    let record = Fixtures.flux()
    #expect(adapter.bid(record, speechMlx)?.tier == .managed)
    #expect(adapter.bid(record, speechUnknown) == nil)
    #expect(adapter.bid(record, textGguf) == nil)
}

@Test func runtimeBundleShipsCompleteAndValid() throws {
    let bundle = try #require(MlxAudioAdapter.bundleDirectory())
    let fm = FileManager.default
    for file in ["main.py", "manifest.toml", "requirements.lock", "sandbox.sb"] {
        #expect(
            fm.fileExists(atPath: bundle.appendingPathComponent(file).path),
            "missing \(file)")
    }
    let manifest = try String(
        contentsOf: bundle.appendingPathComponent("manifest.toml"), encoding: .utf8)
    #expect(manifest.contains("python:mlx-audio"))
    #expect(manifest.contains("network = false"))
    let profile = try String(
        contentsOf: bundle.appendingPathComponent("sandbox.sb"), encoding: .utf8)
    #expect(profile.contains("(deny network*)"))
    #expect(profile.contains("system.sb"))
}
