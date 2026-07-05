import Foundation

public struct SidecarSpec: Sendable {
    public var runtimeID: String
    public var executable: URL
    public var arguments: [String]
    public var environment: [String: String]
    public var workingDirectory: URL?
    public var readyTimeout: Duration
    public var idleTimeout: Duration

    public init(
        runtimeID: String,
        executable: URL,
        arguments: [String],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        readyTimeout: Duration = .seconds(180),
        idleTimeout: Duration = .seconds(120)
    ) {
        self.runtimeID = runtimeID
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.readyTimeout = readyTimeout
        self.idleTimeout = idleTimeout
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

        init(process: Process, stdin: Pipe) {
            self.process = process
            self.stdin = stdin
        }
    }

    private var sidecars: [String: Sidecar] = [:]

    public init() {}

    public func isRunning(_ id: String) -> Bool {
        sidecars[id]?.process.isRunning ?? false
    }

    public func processIdentifier(_ id: String) -> Int32? {
        sidecars[id]?.process.processIdentifier
    }

    public func ensureRunning(_ spec: SidecarSpec) async throws {
        if let existing = sidecars[spec.runtimeID], existing.process.isRunning { return }
        sidecars[spec.runtimeID] = nil

        let process = Process()
        process.executableURL = spec.executable
        process.arguments = spec.arguments
        var env = ProcessInfo.processInfo.environment
        for (key, value) in spec.environment { env[key] = value }
        process.environment = env
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
            throw KernelError.runtimeFailed("sidecar \(id) failed to start: \(tail)")
        }
        if let rate = ready.controlField("sample_rate")?.intValue {
            sidecar.sampleRate = rate
        }
    }

    public func request(
        _ spec: SidecarSpec, _ control: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    try self.send(spec.runtimeID, control)
                    try await self.pump(spec, into: continuation)
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { termination in
                if case .cancelled = termination {
                    Task { await self.kill(spec.runtimeID) }
                }
                task.cancel()
            }
        }
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
                case "done":
                    let seconds = value.objectValue?["seconds"]?.doubleValue
                    continuation.yield(
                        .done(GenerationStats(durationMs: seconds.map { Int($0 * 1000) })))
                    return
                case "error":
                    throw KernelError.runtimeFailed(
                        value.objectValue?["message"]?.stringValue ?? "sidecar error")
                default:
                    continue
                }
            }
        }
        throw KernelError.runtimeFailed(
            "sidecar \(id) exited mid-request: \(stderrTail(id))")
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
            throw KernelError.runtimeFailed("sidecar \(id) is not running")
        }
        let data = try FrameCodec.encode(.control(control))
        try sidecar.stdin.fileHandleForWriting.write(contentsOf: data)
    }

    public func shutdown(_ id: String) async {
        guard let sidecar = sidecars[id] else { return }
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
        if let sidecar = sidecars[id] {
            if sidecar.process.isRunning { sidecar.process.terminate() }
            if let waiter = sidecar.waiter {
                sidecar.waiter = nil
                waiter.resume(returning: nil)
            }
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
