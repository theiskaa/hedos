import Foundation
import Testing

@testable import HedosKernel

private func importsFromBundle(_ bundleName: String, module: String) async throws -> Bool {
    let bundle = try #require(RuntimeBundle.directory(named: bundleName))
    let envDir = try await EnvironmentManager.shared.prepare(
        runtimeID: bundleName, lockfile: bundle.appendingPathComponent("requirements.lock"),
        progress: { _ in })
    let python = envDir.appendingPathComponent("bin/python")
    let process = Process()
    process.executableURL = python
    process.arguments = ["-c", "import \(module)"]
    let stderr = Pipe()
    process.standardError = stderr
    try process.run()
    process.waitUntilExit()
    return process.terminationStatus == 0
}

@Test func mlxVlmLockfileIsImportable() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_MLX_VLM_LIVE"] != nil else { return }
    #expect(try await importsFromBundle("python-mlx-vlm", module: "mlx_vlm"))
}

@Test func embeddingsLockfileIsImportable() async throws {
    guard ProcessInfo.processInfo.environment["HEDOS_EMBEDDINGS_LIVE"] != nil else { return }
    #expect(try await importsFromBundle("python-embeddings", module: "mlx_embeddings"))
}
