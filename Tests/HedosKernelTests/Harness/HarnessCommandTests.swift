import Foundation
import Testing

@testable import HedosKernel

private func commandPlace() throws -> (place: String, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    return (PlaceBoundary.canonical(dir.path), dir)
}

private func runCommand(
    _ argv: [String], place: String, timeout: Int? = nil
) async -> String {
    var arguments: [String: JSONValue] = ["argv": .array(argv.map { .string($0) })]
    if let timeout { arguments["timeout_seconds"] = .int(timeout) }
    return await HarnessActTools.execute(
        ToolCall(name: "run_command", arguments: .object(arguments)),
        place: place,
        context: HarnessActContext(
            sessionID: "s", ask: { _ in .approved(dontAskAgain: false) },
            state: HarnessActState()))
}

@Test func commandWritesInsideThePlaceSucceeds() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await runCommand(["/usr/bin/touch", "made.txt"], place: place)
    #expect(result.contains("exit: 0"))
    #expect(FileManager.default.fileExists(atPath: place + "/made.txt"))
}

@Test func commandWriteOutsideThePlaceIsDeniedBySeatbelt() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let outside = URL(fileURLWithPath: NSHomeDirectory())
        .appendingPathComponent(".hedos-outside-\(UUID().uuidString).txt")
    let result = await runCommand(["/usr/bin/touch", outside.path], place: place)
    #expect(!result.contains("exit: 0"))
    #expect(!FileManager.default.fileExists(atPath: outside.path))
}

@Test func commandHasNoNetwork() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await runCommand(
        [
            "/usr/bin/python3", "-c",
            "import socket; socket.create_connection(('1.1.1.1', 80), 2)",
        ], place: place, timeout: 20)
    #expect(!result.contains("exit: 0"))
}

@Test func commandTimeoutKillsTheTree() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let start = ContinuousClock().now
    let result = await runCommand(["/bin/sleep", "3600"], place: place, timeout: 1)
    let elapsed = ContinuousClock().now - start
    #expect(result.contains("longer than"))
    #expect(elapsed < .seconds(15))
}

@Test func commandStderrSurvivesTheLoopTruncationWithLargeStdout() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let raw = await runCommand(
        [
            "/usr/bin/python3", "-c",
            "import sys; sys.stdout.write('o'*16000); sys.stderr.write('STDERRMARK')",
        ], place: place, timeout: 20)
    let framed = ChatFlow.truncatedToolResult(raw)
    #expect(framed.contains("STDERRMARK"))
}

@Test func declinedCommandNeverRuns() async throws {
    let (place, dir) = try commandPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await HarnessActTools.execute(
        ToolCall(
            name: "run_command",
            arguments: .object(["argv": .array([.string("/usr/bin/touch"), .string("nope.txt")])])),
        place: place,
        context: HarnessActContext(
            sessionID: "s", ask: alwaysDeclineConsent, state: HarnessActState()))
    #expect(result.contains("declined"))
    #expect(!FileManager.default.fileExists(atPath: place + "/nope.txt"))
}
