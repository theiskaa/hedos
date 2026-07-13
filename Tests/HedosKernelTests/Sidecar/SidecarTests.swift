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

@Test func concurrentPreparesCoalesceIntoOneBuild() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let lockA = root.appendingPathComponent("a.lock")
    try Data("packages-a".utf8).write(to: lockA)

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
        try await Task.sleep(for: .milliseconds(200))
        try FileManager.default.createDirectory(
            at: envDir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    }

    async let first = manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    async let second = manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    let (firstURL, secondURL) = try await (first, second)

    #expect(firstURL == secondURL)
    #expect(builds.value == 1)
    #expect(
        FileManager.default.fileExists(
            atPath: firstURL.appendingPathComponent(".hedos-env-ok").path))
}

@Test func preparesForDifferentRuntimesRunIndependently() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let lockA = root.appendingPathComponent("a.lock")
    try Data("packages-a".utf8).write(to: lockA)

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
        try await Task.sleep(for: .milliseconds(200))
        try FileManager.default.createDirectory(
            at: envDir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    }

    async let first = manager.prepare(runtimeID: "python:one", lockfile: lockA) { _ in }
    async let second = manager.prepare(runtimeID: "python:two", lockfile: lockA) { _ in }
    _ = try await (first, second)

    #expect(builds.value == 2)
}

@Test func failedBuildIsRetryable() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let lockA = root.appendingPathComponent("a.lock")
    try Data("packages-a".utf8).write(to: lockA)

    final class Counter: @unchecked Sendable {
        let lock = NSLock()
        var value = 0
        func bump() -> Int {
            lock.lock()
            defer { lock.unlock() }
            value += 1
            return value
        }
    }
    let builds = Counter()
    let manager = EnvironmentManager(root: root) { envDir, _, _, progress in
        let attempt = builds.bump()
        progress("building")
        if attempt == 1 {
            throw KernelError.runtimeFailed("boom")
        }
        try FileManager.default.createDirectory(
            at: envDir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    }

    await #expect(throws: KernelError.self) {
        _ = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    }
    let second = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    #expect(
        FileManager.default.fileExists(
            atPath: second.appendingPathComponent(".hedos-env-ok").path))
    #expect(builds.value == 2)
}

@Test func markerFastPathSkipsBuilder() async throws {
    let root = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    let lockA = root.appendingPathComponent("a.lock")
    try Data("packages-a".utf8).write(to: lockA)

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

    _ = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    #expect(builds.value == 1)
    _ = try await manager.prepare(runtimeID: "python:test", lockfile: lockA) { _ in }
    #expect(builds.value == 1)
}

private func fakeSidecarSpec(
    mode: String = "normal",
    cooperativeCancel: Bool = false, grace: Duration = .seconds(10)
) -> SidecarSpec {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appendingPathComponent("FakeSidecar.py")
    return SidecarSpec(
        runtimeID: "fake-\(mode)-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", script.path, mode],
        readyTimeout: .seconds(15),
        cooperativeCancel: cooperativeCancel,
        cancelGraceTimeout: grace)
}

@Test func supervisorStreamsVectorFromFakeSidecar() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var vectors: [[Double]] = []
    let stream = await supervisor.request(
        spec, .object(["op": .string("embed"), "input": .string("hedos")]))
    for try await chunk in stream {
        if case .vector(let values) = chunk { vectors.append(values) }
    }
    #expect(vectors.count == 1)
    #expect(vectors.first?.first == 5.0)
    await supervisor.shutdownAll()
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

@Test func residencyManagerDrivesSidecarIdleUnload() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    let residency = ResidencyManager(defaultWarmWindow: .milliseconds(400))
    try await supervisor.ensureRunning(spec)
    await residency.register(spec.runtimeID) {
        await supervisor.shutdown(spec.runtimeID)
        return true
    }

    let stream = await supervisor.request(spec, .object(["op": .string("speak")]))
    for try await _ in stream {}
    await residency.scheduleIdleUnload(spec.runtimeID)

    try await Task.sleep(for: .seconds(2))
    #expect(await supervisor.isRunning(spec.runtimeID) == false)

    try await supervisor.ensureRunning(spec)
    let respawnStream = await supervisor.request(spec, .object(["op": .string("speak")]))
    var frames = 0
    for try await chunk in respawnStream {
        if case .audio = chunk { frames += 1 }
    }
    #expect(frames == 3)
    await supervisor.shutdownAll()
}

@Test func concurrentStreamRequestsSerializeWithoutFrameCorruption() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    async let firstFrames: Int = {
        var count = 0
        let stream = await supervisor.request(spec, chatControl("alpha"))
        for try await chunk in stream {
            if case .text = chunk { count += 1 }
        }
        return count
    }()
    async let secondFrames: Int = {
        var count = 0
        let stream = await supervisor.request(spec, chatControl("bravo"))
        for try await chunk in stream {
            if case .text = chunk { count += 1 }
        }
        return count
    }()

    let (first, second) = try await (firstFrames, secondFrames)
    #expect(first == 3)
    #expect(second == 3)
    #expect(await supervisor.isRunning(spec.runtimeID))
    await supervisor.shutdownAll()
}

@Test func cooperativeCancelWatchdogDoesNotKillNextRequest() async throws {
    let spec = fakeSidecarSpec(cooperativeCancel: true, grace: .milliseconds(300))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let cancelled = Task {
        let stream = await supervisor.request(spec, chatControl("slow"))
        for try await chunk in stream {
            if case .text = chunk { break }
        }
    }
    _ = await cancelled.result

    var deltas: [String] = []
    let second = await supervisor.request(spec, chatControl("after"))
    for try await chunk in second {
        if case .text(let delta) = chunk { deltas.append(delta) }
    }
    #expect(deltas.joined() == "after!")
    try await Task.sleep(for: .milliseconds(500))
    #expect(await supervisor.isRunning(spec.runtimeID))
    await supervisor.shutdownAll()
}

private func fakeSidecarScriptPath() -> String {
    URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .appendingPathComponent("FakeSidecar.py").path
}

@Test func frameTimeoutKillsHangingSidecarAndSuccessorStartsClean() async throws {
    let spec = SidecarSpec(
        runtimeID: "fake-hang-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "hang-after-begin"],
        readyTimeout: .seconds(15), frameTimeout: .milliseconds(300))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    await #expect(throws: KernelError.self) {
        for try await _ in await supervisor.request(spec, chatControl("hi")) {}
    }
    var dead = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if await !supervisor.isRunning(spec.runtimeID) { dead = true; break }
    }
    #expect(dead)

    let goodSpec = SidecarSpec(
        runtimeID: spec.runtimeID,
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "normal"],
        readyTimeout: .seconds(15), frameTimeout: .seconds(600))
    try await supervisor.ensureRunning(goodSpec)
    var text = ""
    for try await chunk in await supervisor.request(goodSpec, chatControl("pong")) {
        if case .text(let delta) = chunk { text += delta }
    }
    #expect(text.contains("pong"))
    await supervisor.shutdownAll()
}

@Test func slowButProgressingStreamCompletesWithoutFalseTimeout() async throws {
    let spec = SidecarSpec(
        runtimeID: "fake-slow-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "normal"],
        readyTimeout: .seconds(15), frameTimeout: .milliseconds(400))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var tokens = 0
    for try await chunk in await supervisor.request(spec, chatControl("slow")) {
        if case .text = chunk { tokens += 1 }
    }
    #expect(tokens == 20)
    await supervisor.shutdownAll()
}

@Test func cancelledQueuedRequestNeverSendsItsOp() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let opLog = dir.appendingPathComponent("ops.log")
    let spec = SidecarSpec(
        runtimeID: "fake-oplog-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "normal"],
        environment: ["HEDOS_OP_LOG": opLog.path],
        readyTimeout: .seconds(15), cooperativeCancel: true)
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let aTask = Task {
        for try await _ in await supervisor.request(spec, chatControl("slow")) {}
    }
    for _ in 0..<100 {
        try await Task.sleep(for: .milliseconds(10))
        if let log = try? String(contentsOf: opLog), log.contains("chat") { break }
    }

    let bStream = await supervisor.request(spec, chatControl("again"))
    let bTask = Task { for try await _ in bStream {} }
    try await Task.sleep(for: .milliseconds(20))
    bTask.cancel()

    _ = await aTask.result
    _ = await bTask.result

    for try await _ in await supervisor.request(spec, chatControl("again")) {}

    let log = (try? String(contentsOf: opLog)) ?? ""
    let chatOps = log.split(separator: "\n").filter { $0 == "chat" }.count
    #expect(chatOps == 2)
    await supervisor.shutdownAll()
}

@Test func stallingSidecarDoesNotHeadOfLineBlockOtherSidecars() async throws {
    let supervisor = SidecarSupervisor()
    let stallSpec = SidecarSpec(
        runtimeID: "fake-stall-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "stall-stdin"],
        readyTimeout: .seconds(15))
    let liveSpec = SidecarSpec(
        runtimeID: "fake-live-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", fakeSidecarScriptPath(), "normal"],
        readyTimeout: .seconds(15))
    try await supervisor.ensureRunning(stallSpec)
    try await supervisor.ensureRunning(liveSpec)

    let bigText = String(repeating: "x", count: 4_000_000)
    let stallTask = Task {
        for try await _ in await supervisor.request(stallSpec, chatControl(bigText)) {}
    }

    var text = ""
    for try await chunk in await supervisor.request(liveSpec, chatControl("ping")) {
        if case .text(let delta) = chunk { text += delta }
    }
    #expect(text.contains("ping"))

    stallTask.cancel()
    await supervisor.terminateAll()
}

private func chatControl(_ content: String) -> JSONValue {
    .object([
        "op": .string("chat"),
        "messages": .array([
            .object(["role": .string("user"), "content": .string(content)])
        ]),
    ])
}

@Test func pumpParsesTokenCountsFromChatDoneEvent() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var deltas: [String] = []
    var stats: GenerationStats?
    let stream = await supervisor.request(spec, chatControl("abcdef"))
    for try await chunk in stream {
        switch chunk {
        case .text(let delta): deltas.append(delta)
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(deltas.joined() == "abcdef!")
    #expect(stats?.promptTokens == 1)
    #expect(stats?.completionTokens == 3)
    #expect(stats?.durationMs == 200)

    var speakStats: GenerationStats?
    let speak = await supervisor.request(spec, .object(["op": .string("speak")]))
    for try await chunk in speak {
        if case .done(let s) = chunk { speakStats = s }
    }
    #expect(speakStats?.durationMs == 120)
    #expect(speakStats?.promptTokens == nil)
    #expect(speakStats?.completionTokens == nil)
    await supervisor.shutdownAll()
}

@Test func scrubbedEnvironmentDropsLeakedPythonPathAndHomeButKeepsOverridesAndOtherVars() {
    let base = [
        "PYTHONPATH": "/Users/someone/leaked-project:/other",
        "PYTHONHOME": "/Users/someone/leaked-home",
        "PATH": "/usr/bin:/bin",
    ]
    let scrubbed = SidecarSupervisor.scrubbedEnvironment(base: base, overrides: [:])
    #expect(scrubbed["PYTHONPATH"] == nil)
    #expect(scrubbed["PYTHONHOME"] == nil)
    #expect(scrubbed["PATH"] == "/usr/bin:/bin")

    let withOverride = SidecarSupervisor.scrubbedEnvironment(
        base: base, overrides: ["PYTHONDONTWRITEBYTECODE": "1"])
    #expect(withOverride["PYTHONPATH"] == nil)
    #expect(withOverride["PYTHONDONTWRITEBYTECODE"] == "1")
}

@Test func runProcessScrubbedEnvironmentDropsLeakedPythonPathAndHomeButKeepsOverridesAndOtherVars()
{
    let base = [
        "PYTHONPATH": "/Users/someone/leaked-project:/other",
        "PYTHONHOME": "/Users/someone/leaked-home",
        "PATH": "/usr/bin:/bin",
    ]
    let scrubbed = EnvironmentManager.scrubbedEnvironment(
        base: base, overrides: ["UV_CACHE_DIR": "/override/cache"])
    #expect(scrubbed["PYTHONPATH"] == nil)
    #expect(scrubbed["PYTHONHOME"] == nil)
    #expect(scrubbed["PATH"] == "/usr/bin:/bin")
    #expect(scrubbed["UV_CACHE_DIR"] == "/override/cache")
}

@Test func cooperativeCancelKeepsSidecarWarmAndServesNextRequest() async throws {
    let spec = fakeSidecarSpec(cooperativeCancel: true)
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.request(spec, chatControl("slow"))
        for try await chunk in stream {
            if case .text = chunk { break }
        }
    }
    _ = await consumer.result

    var settled = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if await supervisor.isRunning(spec.runtimeID) {
            settled = true
        } else {
            settled = false
            break
        }
    }
    #expect(settled)

    var deltas: [String] = []
    let second = await supervisor.request(spec, chatControl("again"))
    for try await chunk in second {
        if case .text(let delta) = chunk { deltas.append(delta) }
    }
    #expect(deltas.joined() == "again!")
    await supervisor.shutdownAll()
}

@Test func cooperativeCancelWithoutAckKillsAfterGrace() async throws {
    let spec = fakeSidecarSpec(cooperativeCancel: true, grace: .milliseconds(400))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.request(spec, chatControl("deaf"))
        for try await chunk in stream {
            if case .status = chunk { break }
        }
    }
    _ = await consumer.result

    var dead = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if await !supervisor.isRunning(spec.runtimeID) {
            dead = true
            break
        }
    }
    #expect(dead)
    await supervisor.shutdownAll()
}

@Test func jobCancelWithoutAckKillsAfterGrace() async throws {
    let spec = fakeSidecarSpec(grace: .milliseconds(400))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.jobRequest(
            spec, .object(["op": .string("image"), "prompt": .string("deaf")]))
        for try await event in stream {
            if case .started = event { break }
        }
    }
    _ = await consumer.result

    var dead = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if await !supervisor.isRunning(spec.runtimeID) {
            dead = true
            break
        }
    }
    #expect(dead)
    await supervisor.shutdownAll()
}

@Test func jobCancelWithAckKeepsSidecarWarm() async throws {
    let spec = fakeSidecarSpec(grace: .seconds(10))
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.jobRequest(
            spec, .object(["op": .string("image"), "steps": .int(50)]))
        for try await event in stream {
            if case .progress = event { break }
        }
    }
    _ = await consumer.result

    var settled = false
    for _ in 0..<20 {
        try await Task.sleep(for: .milliseconds(50))
        settled = await supervisor.isRunning(spec.runtimeID)
        if !settled { break }
    }
    #expect(settled)

    var sawResult = false
    let second = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "steps": .int(2)]))
    for try await event in second {
        if case .result = event { sawResult = true }
    }
    #expect(sawResult)
    await supervisor.shutdownAll()
}

@Test func defaultSpecStillKillsOnCancel() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let consumer = Task {
        let stream = await supervisor.request(spec, chatControl("slow"))
        for try await chunk in stream {
            if case .text = chunk { break }
        }
    }
    _ = await consumer.result

    var dead = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if await !supervisor.isRunning(spec.runtimeID) {
            dead = true
            break
        }
    }
    #expect(dead)
    await supervisor.shutdownAll()
}

private func processAlive(_ pid: Int32) -> Bool {
    let ps = Process()
    ps.executableURL = URL(fileURLWithPath: "/bin/ps")
    ps.arguments = ["-p", "\(pid)", "-o", "state="]
    let out = Pipe()
    ps.standardOutput = out
    do { try ps.run() } catch { return false }
    ps.waitUntilExit()
    let state = String(
        decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self
    ).trimmingCharacters(in: .whitespacesAndNewlines)
    return !state.isEmpty && !state.hasPrefix("Z")
}

@Test func terminateAllKillsSidecarsWithoutGracefulShutdown() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)
    let pid = try #require(await supervisor.processIdentifier(spec.runtimeID))
    #expect(processAlive(pid))

    await supervisor.terminateAll()

    #expect(await supervisor.isRunning(spec.runtimeID) == false)
    var alive = true
    for _ in 0..<100 {
        alive = processAlive(pid)
        if !alive { break }
        try await Task.sleep(for: .milliseconds(50))
    }
    #expect(alive == false)
}

@Test func supervisorTimesOutWhenSidecarNeverReady() async throws {
    var spec = fakeSidecarSpec(mode: "never-ready")
    spec = SidecarSpec(
        runtimeID: spec.runtimeID,
        executable: spec.executable,
        arguments: spec.arguments,
        readyTimeout: .milliseconds(800))
    let supervisor = SidecarSupervisor()
    await #expect(throws: KernelError.self) {
        try await supervisor.ensureRunning(spec)
    }
    await supervisor.shutdownAll()
}

@Test func runtimeBundleShipsCompleteAndValid() throws {
    let bundle = try #require(RuntimeBundle.directory(named: "python-mlx-audio"))
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

@Test func restartAfterShutdownSurvivesTheOldGenerationsLateEOF() async throws {
    let spec = fakeSidecarSpec()
    let supervisor = SidecarSupervisor()
    for _ in 0..<3 {
        try await supervisor.ensureRunning(spec)
        await supervisor.shutdown(spec.runtimeID)
        try await supervisor.ensureRunning(spec)
        let stream = await supervisor.request(
            spec, .object(["op": .string("embed"), "input": .string("hedos")]))
        var vectors = 0
        for try await chunk in stream {
            if case .vector = chunk { vectors += 1 }
        }
        #expect(vectors == 1)
        await supervisor.shutdown(spec.runtimeID)
    }
    await supervisor.shutdownAll()
}
