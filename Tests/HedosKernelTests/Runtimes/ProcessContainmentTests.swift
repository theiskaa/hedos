import Foundation
import Testing

@testable import HedosKernel

private func quietChild() throws -> (process: Process, drain: PipeDrain) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sleep")
    process.arguments = ["30"]
    let stdout = Pipe()
    let stderr = Pipe()
    process.standardOutput = stdout
    process.standardError = stderr
    let drain = PipeDrain(stdout: stdout, stderr: stderr)
    try process.run()
    return (process, drain)
}

@Test func pipeDrainCancelDoesNotBlockOnAQuietLiveChild() async throws {
    let (process, drain) = try quietChild()
    defer { if process.isRunning { process.terminate() } }

    let clock = ContinuousClock()
    let start = clock.now
    drain.cancel()
    #expect(clock.now - start < .seconds(2))
}

@Test func terminateProcessTreeKillsAQuietChildPromptly() async throws {
    let (process, _) = try quietChild()

    ProcessContainment.terminateProcessTree(process)
    for _ in 0..<50 where process.isRunning {
        try await Task.sleep(for: .milliseconds(100))
    }
    #expect(!process.isRunning)
}

@Test func cancellingACollectReturnsPromptlyAndKillsTheChild() async throws {
    let (process, drain) = try quietChild()

    let collecting = Task {
        await withTaskCancellationHandler {
            await drain.collect(process: process)
        } onCancel: {
            ProcessContainment.terminateProcessTree(process)
            drain.cancel()
        }
    }
    try await Task.sleep(for: .milliseconds(200))
    let clock = ContinuousClock()
    let start = clock.now
    collecting.cancel()
    _ = await collecting.value
    #expect(clock.now - start < .seconds(5))
    for _ in 0..<50 where process.isRunning {
        try await Task.sleep(for: .milliseconds(100))
    }
    #expect(!process.isRunning)
}
