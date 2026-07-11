import CryptoKit
import Foundation

actor EnvironmentManager {
    typealias Builder = @Sendable (
        _ envDir: URL, _ lockfile: URL, _ cacheDir: URL,
        _ progress: @Sendable (String) -> Void
    ) async throws -> Void

    static let shared = EnvironmentManager(root: Registry.defaultDirectory())

    private let root: URL
    private let builder: Builder
    private var inFlight: [String: Task<URL, Error>] = [:]

    init(root: URL, builder: Builder? = nil) {
        self.root = root
        self.builder = builder ?? EnvironmentManager.uvBuilder
    }

    static func lockHash(_ lockfile: URL) throws -> String {
        let digest = SHA256.hash(data: try Data(contentsOf: lockfile))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    func prepare(
        runtimeID: String, lockfile: URL, progress: @Sendable (String) -> Void
    ) async throws -> URL {
        let hash = try Self.lockHash(lockfile)
        let key = "\(runtimeID)#\(hash)"

        if let existing = inFlight[key] {
            return try await existing.value
        }

        return try await withoutActuallyEscaping(progress) { escapingProgress in
            let task = Task {
                try await self.performPrepare(
                    runtimeID: runtimeID, hash: hash, lockfile: lockfile,
                    progress: escapingProgress)
            }
            inFlight[key] = task
            do {
                let url = try await task.value
                inFlight[key] = nil
                return url
            } catch {
                inFlight[key] = nil
                throw error
            }
        }
    }

    static func sanitizedRuntimeID(_ id: String) -> String {
        String(
            id.unicodeScalars.map { scalar -> Character in
                let allowed =
                    (scalar >= "A" && scalar <= "Z") || (scalar >= "a" && scalar <= "z")
                    || (scalar >= "0" && scalar <= "9") || scalar == "." || scalar == "_"
                    || scalar == "-"
                return allowed ? Character(scalar) : "-"
            })
    }

    private func performPrepare(
        runtimeID: String, hash: String, lockfile: URL, progress: @Sendable (String) -> Void
    ) async throws -> URL {
        let safeID = Self.sanitizedRuntimeID(runtimeID)
        let runtimeDir = root.appendingPathComponent("runtimes/\(safeID)", isDirectory: true)
        let envDir = runtimeDir.appendingPathComponent("envs/\(hash)", isDirectory: true)
        let current = runtimeDir.appendingPathComponent("current")
        let marker = envDir.appendingPathComponent(".hedos-env-ok")
        let fm = FileManager.default

        if fm.fileExists(atPath: marker.path) {
            try relink(current, to: envDir)
            return envDir
        }

        progress("Preparing runtime…")
        if fm.fileExists(atPath: envDir.path) {
            try fm.removeItem(at: envDir)
        }
        try fm.createDirectory(
            at: envDir.deletingLastPathComponent(), withIntermediateDirectories: true)
        let cacheDir = root.appendingPathComponent("uv-cache", isDirectory: true)
        try fm.createDirectory(at: cacheDir, withIntermediateDirectories: true)

        try await builder(envDir, lockfile, cacheDir, progress)
        try Data().write(to: marker)
        try relink(current, to: envDir)
        return envDir
    }

    private func relink(_ link: URL, to destination: URL) throws {
        let fm = FileManager.default
        if (try? fm.destinationOfSymbolicLink(atPath: link.path)) != nil
            || fm.fileExists(atPath: link.path)
        {
            try fm.removeItem(at: link)
        }
        try fm.createSymbolicLink(at: link, withDestinationURL: destination)
    }

    static func uvBinary() -> URL? {
        var candidates = [
            "\(NSHomeDirectory())/.local/bin/uv",
            "/opt/homebrew/bin/uv",
            "/usr/local/bin/uv",
        ]
        if let path = ProcessInfo.processInfo.environment["PATH"] {
            candidates.append(contentsOf: path.split(separator: ":").map { "\($0)/uv" })
        }
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }

    static let uvBuilder: Builder = { envDir, lockfile, cacheDir, progress in
        guard let uv = uvBinary() else {
            throw KernelError.runtimeUnavailable(
                hint: "uv is required to prepare Python runtimes. Install it from astral.sh/uv.")
        }
        progress("Creating Python environment…")
        try await runProcess(
            uv, ["venv", envDir.path, "--python", "3.12"],
            environment: ["UV_CACHE_DIR": cacheDir.path])

        let python = envDir.appendingPathComponent("bin/python").path
        progress("Installing packages…")
        do {
            try await runProcess(
                uv, ["pip", "sync", lockfile.path, "--python", python, "--offline"],
                environment: ["UV_CACHE_DIR": cacheDir.path])
        } catch {
            progress("Downloading packages…")
            try await runProcess(
                uv, ["pip", "sync", lockfile.path, "--python", python],
                environment: ["UV_CACHE_DIR": cacheDir.path])
        }
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

    static func runProcess(
        _ executable: URL, _ arguments: [String], environment extra: [String: String],
        timeout: Duration = .seconds(900)
    ) async throws {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        process.environment = scrubbedEnvironment(
            base: ProcessInfo.processInfo.environment, overrides: extra)
        let unusedStdout = Pipe()
        try? unusedStdout.fileHandleForWriting.close()
        let errPipe = Pipe()
        process.standardOutput = FileHandle.nullDevice
        process.standardError = errPipe
        let drain = PipeDrain(stdout: unusedStdout, stderr: errPipe)

        try process.run()
        let expired = ExpiryMark()
        let watchdog = Task {
            try await Task.sleep(for: timeout)
            expired.mark()
            process.terminate()
            try? await Task.sleep(for: .seconds(5))
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
            }
        }
        let output = await drain.collect(process: process)
        watchdog.cancel()

        let stderrTail = String(decoding: output.stderr, as: UTF8.self).suffix(400)
        let command = "\(executable.lastPathComponent) \(arguments.first ?? "")"
        if expired.wasMarked {
            throw KernelError.runtimeFailed(
                "\(command) timed out after \(timeout.components.seconds)s: \(stderrTail)")
        }
        guard process.terminationStatus == 0 else {
            throw KernelError.runtimeFailed("\(command) failed: \(stderrTail)")
        }
    }
}

private final class ExpiryMark: @unchecked Sendable {
    private let lock = NSLock()
    private var marked = false

    func mark() {
        lock.withLock { marked = true }
    }

    var wasMarked: Bool {
        lock.withLock { marked }
    }
}
