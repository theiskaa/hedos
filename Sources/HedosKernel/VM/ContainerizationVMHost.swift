import Containerization
import CryptoKit
import Foundation

actor ContainerizationVMHost: VMHost {
    static let kernelURL = URL(
        string:
            "https://github.com/kata-containers/kata-containers/releases/download/3.17.0/kata-static-3.17.0-arm64.tar.xz"
    )!
    static let kernelSHA256 =
        "647c7612e6edf789d5e14698c48c99d8bac15ad139ffaa1c8bb7d229f748d181"
    static let vminitReference =
        "ghcr.io/apple/containerization/vminit@sha256:35d594e0634f353fb9113e65c9d743065b6e2e6e6e92a5e999742bc4c8a0d20f"
    static let kernelDownloadMB = 104
    static let rootfsBytes: UInt64 = 12 * 1024 * 1024 * 1024
    static let guestMemoryBytes: UInt64 = 2 * 1024 * 1024 * 1024
    static let guestCPUs = 4

    private let root: URL
    private var running: [String: LinuxContainer] = [:]

    init(directory: URL) {
        self.root = directory.appendingPathComponent("vm", isDirectory: true)
    }

    private var kernelBinary: URL { root.appendingPathComponent("vmlinux.container") }
    private var store: URL { root.appendingPathComponent("store", isDirectory: true) }

    private func containerID(_ runtimeID: String) -> String {
        "vm-" + runtimeID.map { $0.isLetter || $0.isNumber ? $0 : "-" }.map(String.init).joined()
    }

    private func containerDir(_ runtimeID: String) -> URL {
        store.appendingPathComponent("containers", isDirectory: true)
            .appendingPathComponent(containerID(runtimeID), isDirectory: true)
    }

    private func environmentStamp(_ request: VMRunRequest) -> String {
        let material = request.image + "\n" + request.setup.joined(separator: "\n")
        return SHA256.hash(data: Data(material.utf8))
            .map { String(format: "%02x", $0) }.joined()
    }

    func assetState() async -> VMAssetState {
        FileManager.default.fileExists(atPath: kernelBinary.path)
            ? .ready
            : .absent(approxDownloadMB: Self.kernelDownloadMB)
    }

    func provisionAssets(onStatus: (@Sendable (String) async -> Void)?) async throws {
        guard !FileManager.default.fileExists(atPath: kernelBinary.path) else { return }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let tarball = root.appendingPathComponent("kata-static.tar.xz")
        if !FileManager.default.fileExists(atPath: tarball.path) {
            await onStatus?("Downloading the Linux kernel — one time only")
            let (downloaded, _) = try await URLSession.shared.download(from: Self.kernelURL)
            try? FileManager.default.removeItem(at: tarball)
            try FileManager.default.moveItem(at: downloaded, to: tarball)
        }
        let digest = SHA256.hash(data: try Data(contentsOf: tarball))
            .map { String(format: "%02x", $0) }.joined()
        guard digest == Self.kernelSHA256 else {
            try? FileManager.default.removeItem(at: tarball)
            throw KernelError.runtimeFailed(
                "the kernel download did not match its pinned checksum — retry the install")
        }
        await onStatus?("Verifying and unpacking the kernel")
        let untar = Process()
        untar.executableURL = URL(fileURLWithPath: "/usr/bin/tar")
        untar.arguments = [
            "-xJf", tarball.path, "-C", root.path,
            "./opt/kata/share/kata-containers/",
        ]
        try untar.run()
        untar.waitUntilExit()
        guard untar.terminationStatus == 0 else {
            throw KernelError.runtimeFailed("kernel unpack failed")
        }
        let extracted = root.appendingPathComponent(
            "opt/kata/share/kata-containers/vmlinux.container")
        let resolved = extracted.resolvingSymlinksInPath()
        try FileManager.default.copyItem(at: resolved, to: kernelBinary)
        try? FileManager.default.removeItem(at: root.appendingPathComponent("opt"))
        try? FileManager.default.removeItem(at: tarball)
    }

    func environmentReady(_ request: VMRunRequest) async -> Bool {
        let dir = containerDir(request.runtimeID)
        guard
            let stamp = try? String(
                contentsOf: dir.appendingPathComponent(".env-ready"), encoding: .utf8)
        else { return false }
        return stamp == environmentStamp(request)
            && FileManager.default.fileExists(atPath: dir.appendingPathComponent("rootfs.ext4").path)
    }

    func provisionEnvironment(
        _ request: VMRunRequest, onStatus: (@Sendable (String) async -> Void)?
    ) async throws {
        try await provisionAssets(onStatus: onStatus)
        let id = containerID(request.runtimeID)
        let dir = containerDir(request.runtimeID)
        if FileManager.default.fileExists(atPath: dir.path) {
            try FileManager.default.removeItem(at: dir)
        }
        let kernel = Containerization.Kernel(path: kernelBinary, platform: .linuxArm)
        let needsNetwork = !request.setup.isEmpty
        var manager = try await ContainerManager(
            kernel: kernel,
            initfsReference: Self.vminitReference,
            root: store,
            network: needsNetwork ? try VmnetNetwork() : nil
        )
        await onStatus?("Fetching the runtime image")
        let container = try await manager.create(
            id,
            reference: request.image,
            rootfsSizeInBytes: Self.rootfsBytes,
            networking: needsNetwork
        ) { config in
            config.cpus = Self.guestCPUs
            config.memoryInBytes = Self.guestMemoryBytes
            config.process.arguments = ["sleep", "infinity"]
        }
        try await container.create()
        try await container.start()
        running[request.runtimeID] = container
        defer { running[request.runtimeID] = nil }
        do {
            for (index, command) in request.setup.enumerated() {
                await onStatus?(
                    "Preparing the runtime environment (\(index + 1)/\(request.setup.count))")
                let result = try await exec(container, ["sh", "-c", command])
                guard result.exitCode == 0 else {
                    throw KernelError.runtimeFailed(
                        "runtime setup failed: \(lastLine(result.stderr + result.stdout))")
                }
            }
            try await container.stop()
        } catch {
            try? await container.stop()
            if needsNetwork { try? manager.releaseNetwork(id) }
            try? FileManager.default.removeItem(at: dir)
            throw error
        }
        if needsNetwork { try? manager.releaseNetwork(id) }
        try environmentStamp(request).write(
            to: dir.appendingPathComponent(".env-ready"), atomically: true, encoding: .utf8)
    }

    func run(_ request: VMRunRequest) async throws -> VMRunResult {
        let id = containerID(request.runtimeID)
        let rootfs = containerDir(request.runtimeID).appendingPathComponent("rootfs.ext4")
        guard FileManager.default.fileExists(atPath: rootfs.path) else {
            throw KernelError.runtimeUnavailable(
                hint: "the contained runtime environment is not prepared yet")
        }
        let kernel = Containerization.Kernel(path: kernelBinary, platform: .linuxArm)
        var manager = try await ContainerManager(
            kernel: kernel,
            initfsReference: Self.vminitReference,
            root: store
        )
        let image = try await manager.imageStore.get(reference: request.image, pull: false)
        let container = try await manager.create(
            id,
            image: image,
            rootfs: .block(format: "ext4", source: rootfs.path, destination: "/", options: []),
            networking: false
        ) { config in
            config.cpus = Self.guestCPUs
            config.memoryInBytes = Self.guestMemoryBytes
            config.process.arguments = ["sleep", "infinity"]
            if let model = request.modelPath {
                config.mounts.append(
                    .share(source: model, destination: VMGuestPath.model, options: ["ro"]))
            }
            if let resources = request.resourcesPath {
                config.mounts.append(
                    .share(
                        source: resources, destination: VMGuestPath.resources, options: ["ro"]))
            }
            config.mounts.append(
                .share(source: request.workdir, destination: VMGuestPath.workdir))
            config.mounts.append(
                .share(source: request.outputs, destination: VMGuestPath.outputs))
        }
        try await container.create()
        try await container.start()
        running[request.runtimeID] = container
        defer { running[request.runtimeID] = nil }
        do {
            let result = try await exec(container, request.arguments, timeout: 1800)
            try await container.stop()
            return result
        } catch {
            try? await container.stop()
            throw error
        }
    }

    func cancel(runtimeID: String) async {
        if let container = running.removeValue(forKey: runtimeID) {
            try? await container.stop()
        }
    }

    private func exec(
        _ container: LinuxContainer, _ arguments: [String], timeout: Int64 = 900
    ) async throws -> VMRunResult {
        let stdout = VMBufferWriter()
        let stderr = VMBufferWriter()
        let process = try await container.exec(UUID().uuidString) { config in
            config.arguments = arguments
            config.stdout = stdout
            config.stderr = stderr
        }
        try await process.start()
        let status = try await process.wait(timeoutInSeconds: timeout)
        try await process.delete()
        return VMRunResult(exitCode: status.exitCode, stdout: stdout.text, stderr: stderr.text)
    }

    private func lastLine(_ text: String) -> String {
        text.split(separator: "\n").last.map(String.init) ?? text
    }
}

final class VMBufferWriter: Writer, @unchecked Sendable {
    static let maxBytesPerStream = 16 * 1024 * 1024

    private let lock = NSLock()
    private var buffer = Data()
    private let maxBytes: Int

    init(maxBytes: Int = VMBufferWriter.maxBytesPerStream) {
        self.maxBytes = maxBytes
    }

    func write(_ data: Data) throws {
        lock.lock()
        defer { lock.unlock() }
        guard buffer.count + data.count <= maxBytes else {
            throw KernelError.runtimeFailed(
                "the contained runtime wrote more than 16 MiB of output")
        }
        buffer.append(data)
    }

    func close() throws {}

    var text: String {
        lock.lock()
        defer { lock.unlock() }
        return String(data: buffer, encoding: .utf8) ?? ""
    }
}
