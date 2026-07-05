import CryptoKit
import Foundation

public actor EnvironmentManager {
    public typealias Builder = @Sendable (
        _ envDir: URL, _ lockfile: URL, _ cacheDir: URL,
        _ progress: @Sendable (String) -> Void
    ) async throws -> Void

    public static let shared = EnvironmentManager(root: Registry.defaultDirectory())

    private let root: URL
    private let builder: Builder

    public init(root: URL, builder: Builder? = nil) {
        self.root = root
        self.builder = builder ?? EnvironmentManager.uvBuilder
    }

    public static func lockHash(_ lockfile: URL) throws -> String {
        let digest = SHA256.hash(data: try Data(contentsOf: lockfile))
        return digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    public func prepare(
        runtimeID: String, lockfile: URL, progress: @Sendable (String) -> Void
    ) async throws -> URL {
        let hash = try Self.lockHash(lockfile)
        let safeID = runtimeID.replacingOccurrences(of: ":", with: "-")
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

    public static func uvBinary() -> URL? {
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

    static func runProcess(
        _ executable: URL, _ arguments: [String], environment extra: [String: String]
    ) async throws {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        var env = ProcessInfo.processInfo.environment
        for (key, value) in extra { env[key] = value }
        process.environment = env
        let errPipe = Pipe()
        process.standardOutput = FileHandle.nullDevice
        process.standardError = errPipe

        try process.run()
        await withCheckedContinuation { continuation in
            process.terminationHandler = { _ in continuation.resume() }
        }
        guard process.terminationStatus == 0 else {
            let stderr = String(
                decoding: errPipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            throw KernelError.runtimeFailed(
                "\(executable.lastPathComponent) \(arguments.first ?? "") failed: \(stderr.suffix(400))")
        }
    }
}
