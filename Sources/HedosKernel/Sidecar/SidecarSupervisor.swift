import Foundation

public actor SidecarSupervisor {
    public static let shared = SidecarSupervisor()

    final class Sidecar {
        let process: Process
        let stdin: Pipe
        var decoder = FrameCodec.Decoder()
        var buffered: [Frame] = []
        var waiter: CheckedContinuation<Frame?, Never>?
        var eof = false
        var sampleRate = SidecarSpec.defaultSampleRate
        var stderrTail = ""
        var busy = false
        var jobSession: UUID?
        var jobOpSent = false
        var pending: [CheckedContinuation<Void, Never>] = []
        var lastFrameAt = ContinuousClock().now
        let writeQueue: DispatchQueue

        init(process: Process, stdin: Pipe) {
            self.process = process
            self.stdin = stdin
            self.writeQueue = DispatchQueue(label: "hedos.sidecar.stdin")
        }
    }

    var sidecars: [String: Sidecar] = [:]
    private var cancelWatchdogs: [String: (session: UUID, task: Task<Void, Never>)] = [:]
    private var frameWatchdogs: [String: (session: UUID, task: Task<Void, Never>)] = [:]

    public init() {
        signal(SIGPIPE, SIG_IGN)
    }

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

        guard let ready = await nextReadyFrame(id, timeout: spec.readyTimeout),
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
                    } else {
                        Task { await self.kill(spec.runtimeID) }
                    }
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
        armFrameWatchdog(id, session: session, timeout: spec.frameTimeout)
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
                await self.settleStream(spec.runtimeID, session: session)
            }
            continuation.onTermination = { termination in
                guard case .cancelled = termination else { return }
                task.cancel()
                Task {
                    await self.cancelJob(
                        spec.runtimeID, session: session, grace: spec.cancelGraceTimeout)
                }
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
        armFrameWatchdog(id, session: session, timeout: spec.frameTimeout)
        try await pumpJob(spec, into: continuation)
    }

    private func cancelJob(_ id: String, session: UUID, grace: Duration) {
        guard let sidecar = sidecars[id],
            sidecar.busy,
            sidecar.jobSession == session,
            sidecar.jobOpSent
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

    private func acquireExclusive(_ id: String, session: UUID) async {
        while let sidecar = sidecars[id], sidecar.busy {
            await withCheckedContinuation { sidecar.pending.append($0) }
        }
        guard !Task.isCancelled, let sidecar = sidecars[id] else { return }
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
        clearFrameWatchdog(id)
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

    private func ingest(_ data: Data, for id: String) {
        guard let sidecar = sidecars[id] else { return }
        do {
            let frames = try sidecar.decoder.append(data)
            for frame in frames {
                sidecar.lastFrameAt = ContinuousClock().now
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

    func nextFrame(_ id: String) async -> Frame? {
        guard let sidecar = sidecars[id] else { return nil }
        if !sidecar.buffered.isEmpty {
            return sidecar.buffered.removeFirst()
        }
        if sidecar.eof { return nil }
        return await withCheckedContinuation { continuation in
            sidecar.waiter = continuation
        }
    }

    private func nextReadyFrame(_ id: String, timeout: Duration) async -> Frame? {
        let timeoutTask = Task { [weak self] in
            try? await Task.sleep(for: timeout)
            if Task.isCancelled { return }
            await self?.expireWaiter(id)
        }
        let frame = await nextFrame(id)
        timeoutTask.cancel()
        return frame
    }

    private func expireWaiter(_ id: String) {
        guard let sidecar = sidecars[id], let waiter = sidecar.waiter else { return }
        sidecar.waiter = nil
        waiter.resume(returning: nil)
    }

    private func armFrameWatchdog(_ id: String, session: UUID, timeout: Duration) {
        frameWatchdogs[id]?.task.cancel()
        frameWatchdogs[id] = (
            session,
            Task { [weak self] in
                await self?.runFrameWatchdog(id, session: session, timeout: timeout)
            }
        )
    }

    private func runFrameWatchdog(_ id: String, session: UUID, timeout: Duration) async {
        while !Task.isCancelled {
            try? await Task.sleep(for: timeout)
            if Task.isCancelled { return }
            guard let sidecar = sidecars[id], sidecar.busy, sidecar.jobSession == session
            else { return }
            if ContinuousClock().now - sidecar.lastFrameAt >= timeout, let waiter = sidecar.waiter {
                sidecar.waiter = nil
                waiter.resume(returning: nil)
            }
        }
    }

    private func clearFrameWatchdog(_ id: String) {
        frameWatchdogs[id]?.task.cancel()
        frameWatchdogs[id] = nil
    }

    private func appendStderr(_ text: String, for id: String) {
        guard let sidecar = sidecars[id] else { return }
        sidecar.stderrTail = String((sidecar.stderrTail + text).suffix(2000))
    }

    func stderrTail(_ id: String) -> String {
        sidecars[id]?.stderrTail.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }

    private func send(_ id: String, _ control: JSONValue) throws {
        guard let sidecar = sidecars[id], sidecar.process.isRunning else {
            throw KernelError.sidecarDied(runtimeID: id, detail: "is not running")
        }
        let data = try FrameCodec.encode(.control(control))
        let handle = sidecar.stdin.fileHandleForWriting
        sidecar.writeQueue.async { [weak self] in
            do {
                try handle.write(contentsOf: data)
            } catch {
                Task { await self?.kill(id) }
            }
        }
    }

    public func shutdown(_ id: String) async {
        guard let sidecar = sidecars[id] else { return }
        clearWatchdog(id)
        clearFrameWatchdog(id)
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

    func kill(_ id: String) {
        clearWatchdog(id)
        clearFrameWatchdog(id)
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
