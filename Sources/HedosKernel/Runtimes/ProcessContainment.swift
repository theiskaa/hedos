import Darwin
import Foundation

enum ProcessContainment {
    static func descendantPIDs(of pid: pid_t) -> [pid_t] {
        let ps = Process()
        ps.executableURL = URL(fileURLWithPath: "/bin/ps")
        ps.arguments = ["-A", "-o", "pid=,ppid="]
        let stdout = Pipe()
        ps.standardOutput = stdout
        ps.standardError = Pipe()
        guard (try? ps.run()) != nil else { return [] }
        let data = (try? stdout.fileHandleForReading.readToEnd()) ?? nil
        ps.waitUntilExit()
        guard let data else { return [] }

        var parents: [pid_t: [pid_t]] = [:]
        for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
            let fields = line.split(separator: " ").compactMap { pid_t($0) }
            guard fields.count == 2 else { continue }
            parents[fields[1], default: []].append(fields[0])
        }

        var result: [pid_t] = []
        var frontier: [pid_t] = [pid]
        while !frontier.isEmpty {
            let current = frontier.removeFirst()
            let children = parents[current] ?? []
            result.append(contentsOf: children)
            frontier.append(contentsOf: children)
        }
        return result
    }

    static func terminateProcessTree(_ process: Process, grace: Duration = .milliseconds(500)) {
        let pid = process.processIdentifier
        guard pid > 0 else {
            if process.isRunning { process.terminate() }
            return
        }
        Task.detached {
            let descendants = descendantPIDs(of: pid)
            kill(pid, SIGTERM)
            for child in descendants { kill(child, SIGTERM) }
            if process.isRunning { process.terminate() }
            try? await Task.sleep(for: grace)
            for _ in 0..<2 {
                for child in descendantPIDs(of: pid) where kill(child, 0) == 0 {
                    kill(child, SIGKILL)
                }
                if process.isRunning {
                    kill(pid, SIGKILL)
                }
            }
        }
    }
}

actor TimeoutFlag {
    private(set) var didFire = false
    func fire() { didFire = true }
}

final class PipeDrain: @unchecked Sendable {
    static let maxBytesPerStream = 16 * 1024 * 1024

    private let stdout: Pipe
    private let stderr: Pipe
    private let maxBytes: Int
    private let lock = NSLock()
    private var out = Data()
    private var err = Data()
    private var exceeded = false
    private let onCapExceeded: @Sendable () -> Void

    init(
        stdout: Pipe, stderr: Pipe, maxBytes: Int = PipeDrain.maxBytesPerStream,
        onCapExceeded: @escaping @Sendable () -> Void = {}
    ) {
        self.stdout = stdout
        self.stderr = stderr
        self.maxBytes = maxBytes
        self.onCapExceeded = onCapExceeded
    }

    func collect(process: Process) async -> (stdout: Data, stderr: Data) {
        stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.append(handle.availableData, isStdout: true)
        }
        stderr.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.append(handle.availableData, isStdout: false)
        }
        await withCheckedContinuation { continuation in
            let resumed = ResumeOnce(continuation)
            process.terminationHandler = { _ in resumed.fire() }
            if !process.isRunning { resumed.fire() }
        }
        cancel()
        return lock.withLock { (out, err) }
    }

    func cancel() {
        stdout.fileHandleForReading.readabilityHandler = nil
        stderr.fileHandleForReading.readabilityHandler = nil
        append(Self.drainWithoutBlocking(stdout.fileHandleForReading), isStdout: true)
        append(Self.drainWithoutBlocking(stderr.fileHandleForReading), isStdout: false)
    }

    private static func drainWithoutBlocking(_ handle: FileHandle) -> Data {
        let fd = handle.fileDescriptor
        let flags = fcntl(fd, F_GETFL)
        guard flags >= 0, fcntl(fd, F_SETFL, flags | O_NONBLOCK) >= 0 else { return Data() }
        var collected = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while collected.count < 1 << 20 {
            let count = read(fd, &buffer, buffer.count)
            guard count > 0 else { break }
            collected.append(contentsOf: buffer[0..<count])
        }
        return collected
    }

    private func append(_ data: Data, isStdout: Bool) {
        guard !data.isEmpty else { return }
        var didExceed = false
        lock.withLock {
            guard !exceeded else { return }
            if isStdout {
                out.append(data)
                if out.count > maxBytes { exceeded = true }
            } else {
                err.append(data)
                if err.count > maxBytes { exceeded = true }
            }
            didExceed = exceeded
        }
        if didExceed { onCapExceeded() }
    }
}

final class ResumeOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<Void, Never>?

    init(_ continuation: CheckedContinuation<Void, Never>) {
        self.continuation = continuation
    }

    func fire() {
        lock.lock()
        let c = continuation
        continuation = nil
        lock.unlock()
        c?.resume()
    }
}
