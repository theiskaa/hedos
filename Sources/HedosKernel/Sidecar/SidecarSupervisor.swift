import Foundation

public struct SidecarSpec: Sendable {
    public var runtimeID: String
    public var executable: URL
    public var arguments: [String]
    public var environment: [String: String]
    public var workingDirectory: URL?
    public var readyTimeout: Duration
    public var cooperativeCancel: Bool
    public var cancelGraceTimeout: Duration

    public init(
        runtimeID: String,
        executable: URL,
        arguments: [String],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        readyTimeout: Duration = .seconds(180),
        cooperativeCancel: Bool = false,
        cancelGraceTimeout: Duration = .seconds(10)
    ) {
        self.runtimeID = runtimeID
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.readyTimeout = readyTimeout
        self.cooperativeCancel = cooperativeCancel
        self.cancelGraceTimeout = cancelGraceTimeout
    }
}

public actor SidecarSupervisor {
    public static let shared = SidecarSupervisor()

    private final class Sidecar {
        let process: Process
        let stdin: Pipe
        var decoder = FrameCodec.Decoder()
        var buffered: [Frame] = []
        var waiter: CheckedContinuation<Frame?, Never>?
        var waiterGeneration = 0
        var eof = false
        var sampleRate = 24000
        var stderrTail = ""
        var busy = false
        var jobSession: UUID?
        var jobOpSent = false
        var pending: [CheckedContinuation<Void, Never>] = []

        init(process: Process, stdin: Pipe) {
            self.process = process
            self.stdin = stdin
        }
    }

    private var sidecars: [String: Sidecar] = [:]
    private var cancelWatchdogs: [String: (session: UUID, task: Task<Void, Never>)] = [:]

    public init() {}

    public func isRunning(_ id: String) -> Bool {
        sidecars[id]?.process.isRunning ?? false
    }

    public func processIdentifier(_ id: String) -> Int32? {
        sidecars[id]?.process.processIdentifier
    }

    static func scrubbedEnvironment(
        base: [String: String], overrides: [String: String]
    ) -> [String: String] {
        var env = base
        env.removeValue(forKey: "PYTHONPATH")
        env.removeValue(forKey: "PYTHONHOME")
        for (key, value) in overrides { env[key] = value }
        return env
    }

    public func ensureRunning(_ spec: SidecarSpec) async throws {
        if let existing = sidecars[spec.runtimeID], existing.process.isRunning { return }
        sidecars[spec.runtimeID] = nil

        let process = Process()
        process.executableURL = spec.executable
        process.arguments = spec.arguments
        process.environment = Self.scrubbedEnvironment(
            base: ProcessInfo.processInfo.environment, overrides: spec.environment)
        if let workingDirectory = spec.workingDirectory {
            process.currentDirectoryURL = workingDirectory
        }

        let stdin = Pipe()
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr

        let sidecar = Sidecar(process: process, stdin: stdin)
        let id = spec.runtimeID

        let (chunks, chunkContinuation) = AsyncStream<Data>.makeStream(
            bufferingPolicy: .unbounded)
        stdout.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                chunkContinuation.finish()
                return
            }
            chunkContinuation.yield(data)
        }
        Task {
            for await chunk in chunks {
                await self.ingest(chunk, for: id)
            }
            await self.markEOF(id)
        }
        stderr.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            guard !data.isEmpty else {
                handle.readabilityHandler = nil
                return
            }
            Task { await self.appendStderr(String(decoding: data, as: UTF8.self), for: id) }
        }

        try process.run()
        sidecars[id] = sidecar

        guard let ready = await nextFrame(id, timeout: spec.readyTimeout),
            case .control(let value) = ready,
            value.objectValue?["event"]?.stringValue == "ready"
        else {
            let tail = stderrTail(id)
            kill(id)
            throw KernelError.sidecarDied(
                runtimeID: id, detail: "failed to start: \(ManifestSupport.errorSummary(tail))")
        }
        if let rate = ready.controlField("sample_rate")?.intValue {
            sidecar.sampleRate = rate
        }
    }

    public func request(
        _ spec: SidecarSpec, _ control: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let session = UUID()
            let task = Task {
                do {
                    try await self.runStream(
                        spec, control, session: session, into: continuation)
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
                await self.settleStream(spec.runtimeID, session: session)
            }
            continuation.onTermination = { termination in
                if case .cancelled = termination {
                    if spec.cooperativeCancel {
                        Task {
                            await self.cancelStream(
                                spec.runtimeID, session: session,
                                grace: spec.cancelGraceTimeout)
                        }
                        return
                    }
                    Task { await self.kill(spec.runtimeID) }
                }
                task.cancel()
            }
        }
    }

    private func runStream(
        _ spec: SidecarSpec, _ control: JSONValue, session: UUID,
        into continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        await acquireExclusive(id, session: session)
        defer { releaseExclusive(id, session: session) }
        try Task.checkCancellation()
        try send(id, control)
        markJobOpSent(id, session: session)
        try await pump(spec, into: continuation)
    }

    private func cancelStream(_ id: String, session: UUID, grace: Duration) {
        guard let sidecar = sidecars[id], sidecar.busy,
            sidecar.jobSession == session, sidecar.jobOpSent
        else { return }
        try? send(id, .object(["op": .string("cancel")]))
        cancelWatchdogs[id]?.task.cancel()
        cancelWatchdogs[id] = (
            session,
            Task {
                try? await Task.sleep(for: grace)
                await self.expireCancelWatchdog(id, session: session)
            }
        )
    }

    private func expireCancelWatchdog(_ id: String, session: UUID) {
        guard !Task.isCancelled,
            let watchdog = cancelWatchdogs[id], watchdog.session == session
        else { return }
        cancelWatchdogs[id] = nil
        guard let sidecar = sidecars[id], sidecar.busy, sidecar.jobSession == session else {
            return
        }
        kill(id)
    }

    private func settleStream(_ id: String, session: UUID) {
        guard let watchdog = cancelWatchdogs[id], watchdog.session == session else { return }
        watchdog.task.cancel()
        cancelWatchdogs[id] = nil
    }

    private func clearWatchdog(_ id: String) {
        cancelWatchdogs[id]?.task.cancel()
        cancelWatchdogs[id] = nil
    }

    public func jobRequest(
        _ spec: SidecarSpec, _ control: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let session = UUID()
            let task = Task {
                do {
                    try await self.runJob(spec, control, session: session, into: continuation)
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { termination in
                guard case .cancelled = termination else { return }
                task.cancel()
                Task { await self.cancelJob(spec.runtimeID, session: session) }
            }
        }
    }

    private func runJob(
        _ spec: SidecarSpec, _ control: JSONValue, session: UUID,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        await acquireExclusive(id, session: session)
        defer { releaseExclusive(id, session: session) }
        try Task.checkCancellation()
        try send(id, control)
        markJobOpSent(id, session: session)
        try await pumpJob(spec, into: continuation)
    }

    private func cancelJob(_ id: String, session: UUID) {
        guard let sidecar = sidecars[id],
            sidecar.busy,
            sidecar.jobSession == session,
            sidecar.jobOpSent
        else { return }
        try? send(id, .object(["op": .string("cancel")]))
    }

    private func acquireExclusive(_ id: String, session: UUID) async {
        while let sidecar = sidecars[id], sidecar.busy {
            await withCheckedContinuation { sidecar.pending.append($0) }
        }
        guard let sidecar = sidecars[id] else { return }
        sidecar.busy = true
        sidecar.jobSession = session
        sidecar.jobOpSent = false
    }

    private func markJobOpSent(_ id: String, session: UUID) {
        guard let sidecar = sidecars[id], sidecar.jobSession == session else { return }
        sidecar.jobOpSent = true
    }

    private func releaseExclusive(_ id: String, session: UUID) {
        guard let sidecar = sidecars[id], sidecar.jobSession == session else { return }
        sidecar.jobSession = nil
        sidecar.jobOpSent = false
        sidecar.busy = false
        resumePending(sidecar)
    }

    private func resumePending(_ sidecar: Sidecar) {
        let waiters = sidecar.pending
        sidecar.pending = []
        for waiter in waiters {
            waiter.resume()
        }
    }

    private func pumpJob(
        _ spec: SidecarSpec,
        into continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        while let frame = await nextFrame(id, timeout: .seconds(600)) {
            guard case .control(let value) = frame else { continue }
            switch value.objectValue?["event"]?.stringValue {
            case "begin":
                continuation.yield(.started)
            case "step":
                let step = value.objectValue?["n"]?.intValue ?? 0
                let total = value.objectValue?["total"]?.intValue ?? 0
                continuation.yield(.progress(step: step, totalSteps: total))
            case "preview":
                if let data = await nextBinaryFrame(id) {
                    continuation.yield(.preview(data))
                }
            case "image":
                let format = value.objectValue?["format"]?.stringValue ?? "png"
                if let data = await nextBinaryFrame(id) {
                    continuation.yield(.result(data: data, fileExtension: format))
                }
            case "done":
                return
            case "cancelled":
                throw CancellationError()
            case "error":
                throw KernelError.runtimeFailed(
                    value.objectValue?["message"]?.stringValue ?? "sidecar error")
            default:
                continue
            }
        }
        throw KernelError.sidecarDied(
            runtimeID: id,
            detail: "stopped unexpectedly: \(ManifestSupport.errorSummary(stderrTail(id)))")
    }

    private func nextBinaryFrame(_ id: String) async -> Data? {
        guard let frame = await nextFrame(id, timeout: .seconds(600)),
            case .audio(let data) = frame
        else { return nil }
        return data
    }

    private func pump(
        _ spec: SidecarSpec,
        into continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        let id = spec.runtimeID
        let sampleRate = sidecars[id]?.sampleRate ?? 24000

        while let frame = await nextFrame(id, timeout: .seconds(600)) {
            switch frame {
            case .audio(let data):
                continuation.yield(.audio(AudioFrame(data: data, sampleRate: sampleRate)))
            case .control(let value):
                switch value.objectValue?["event"]?.stringValue {
                case "begin":
                    continuation.yield(.status("generating"))
                case "text":
                    continuation.yield(.text(value.objectValue?["text"]?.stringValue ?? ""))
                case "done":
                    let seconds = value.objectValue?["seconds"]?.doubleValue
                    continuation.yield(
                        .done(
                            GenerationStats(
                                promptTokens: value.objectValue?["prompt_tokens"]?.intValue,
                                completionTokens: value.objectValue?["completion_tokens"]?
                                    .intValue,
                                durationMs: seconds.map { Int($0 * 1000) })))
                    return
                case "cancelled":
                    throw CancellationError()
                case "error":
                    throw KernelError.runtimeFailed(
                        value.objectValue?["message"]?.stringValue ?? "sidecar error")
                default:
                    continue
                }
            }
        }
        throw KernelError.sidecarDied(
            runtimeID: id,
            detail: "stopped unexpectedly: \(ManifestSupport.errorSummary(stderrTail(id)))")
    }

    private func ingest(_ data: Data, for id: String) {
        guard let sidecar = sidecars[id] else { return }
        do {
            let frames = try sidecar.decoder.append(data)
            for frame in frames {
                if let waiter = sidecar.waiter {
                    sidecar.waiter = nil
                    waiter.resume(returning: frame)
                } else {
                    sidecar.buffered.append(frame)
                }
            }
        } catch {
            kill(id)
        }
    }

    private func markEOF(_ id: String) {
        guard let sidecar = sidecars[id] else { return }
        sidecar.eof = true
        if let waiter = sidecar.waiter {
            sidecar.waiter = nil
            waiter.resume(returning: nil)
        }
        resumePending(sidecar)
    }

    private func nextFrame(_ id: String, timeout: Duration) async -> Frame? {
        guard let sidecar = sidecars[id] else { return nil }
        if !sidecar.buffered.isEmpty {
            return sidecar.buffered.removeFirst()
        }
        if sidecar.eof { return nil }

        sidecar.waiterGeneration += 1
        let generation = sidecar.waiterGeneration
        let timeoutTask = Task {
            try? await Task.sleep(for: timeout)
            guard !Task.isCancelled else { return }
            await self.timeoutWaiter(id, generation: generation)
        }
        let frame = await withCheckedContinuation { continuation in
            sidecar.waiter = continuation
        }
        timeoutTask.cancel()
        return frame
    }

    private func timeoutWaiter(_ id: String, generation: Int) {
        guard let sidecar = sidecars[id],
            sidecar.waiterGeneration == generation,
            let waiter = sidecar.waiter
        else { return }
        sidecar.waiter = nil
        waiter.resume(returning: nil)
    }

    private func appendStderr(_ text: String, for id: String) {
        guard let sidecar = sidecars[id] else { return }
        sidecar.stderrTail = String((sidecar.stderrTail + text).suffix(2000))
    }

    private func stderrTail(_ id: String) -> String {
        sidecars[id]?.stderrTail.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    private func send(_ id: String, _ control: JSONValue) throws {
        guard let sidecar = sidecars[id], sidecar.process.isRunning else {
            throw KernelError.sidecarDied(runtimeID: id, detail: "is not running")
        }
        let data = try FrameCodec.encode(.control(control))
        try sidecar.stdin.fileHandleForWriting.write(contentsOf: data)
    }

    public func shutdown(_ id: String) async {
        guard let sidecar = sidecars[id] else { return }
        clearWatchdog(id)
        if sidecar.process.isRunning {
            try? send(id, .object(["op": .string("shutdown")]))
            for _ in 0..<30 where sidecar.process.isRunning {
                try? await Task.sleep(for: .milliseconds(100))
            }
            if sidecar.process.isRunning { sidecar.process.terminate() }
        }
        markEOF(id)
        sidecars[id] = nil
    }

    public func shutdownAll() async {
        for id in Array(sidecars.keys) {
            await shutdown(id)
        }
    }

    public func terminateAll() {
        for id in Array(sidecars.keys) {
            kill(id)
        }
    }

    private func kill(_ id: String) {
        clearWatchdog(id)
        if let sidecar = sidecars[id] {
            if sidecar.process.isRunning { sidecar.process.terminate() }
            if let waiter = sidecar.waiter {
                sidecar.waiter = nil
                waiter.resume(returning: nil)
            }
            resumePending(sidecar)
        }
        sidecars[id] = nil
    }
}

extension Frame {
    fileprivate func controlField(_ key: String) -> JSONValue? {
        guard case .control(let value) = self else { return nil }
        return value.objectValue?[key]
    }
}
