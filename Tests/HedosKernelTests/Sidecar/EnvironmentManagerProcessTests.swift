import Foundation
import Synchronization
import Testing

@testable import HedosKernel

private final class AttemptCounter: Sendable {
    private let attempts = Mutex(0)

    func next() -> Int {
        attempts.withLock {
            $0 += 1
            return $0
        }
    }

    var count: Int {
        attempts.withLock { $0 }
    }
}

@Test func childFillingStderrStillTerminates() async throws {
    try await EnvironmentManager.runProcess(
        URL(fileURLWithPath: "/bin/sh"),
        ["-c", "dd if=/dev/zero bs=1024 count=256 2>/dev/null | tr '\\0' x >&2"],
        environment: [:],
        timeout: .seconds(30))
}

@Test func hungChildTimesOutWithHonestError() async throws {
    do {
        try await EnvironmentManager.runProcess(
            URL(fileURLWithPath: "/bin/sleep"), ["60"],
            environment: [:],
            timeout: .seconds(1))
        Issue.record("a hung child must throw")
    } catch let KernelError.runtimeFailed(message) {
        #expect(message.contains("timed out after 1s"))
        #expect(message.contains("sleep"))
    }
}

@Test func failedPrepareClearsInFlightAndRetries() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let lockfile = dir.appendingPathComponent("uv.lock")
    try Data("deps".utf8).write(to: lockfile)

    let attempts = AttemptCounter()
    let manager = EnvironmentManager(root: dir) { envDir, _, _, _ in
        if attempts.next() == 1 {
            throw KernelError.runtimeFailed("first attempt dies")
        }
        try FileManager.default.createDirectory(at: envDir, withIntermediateDirectories: true)
    }

    await #expect(throws: (any Error).self) {
        _ = try await manager.prepare(runtimeID: "python:test", lockfile: lockfile) { _ in }
    }
    let env = try await manager.prepare(runtimeID: "python:test", lockfile: lockfile) { _ in }
    #expect(FileManager.default.fileExists(atPath: env.path))
    #expect(attempts.count == 2)
}
