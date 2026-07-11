import Foundation
import Testing

@testable import HedosKernel

private func consentPlace() throws -> (place: String, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    try Data("keep\n".utf8).write(to: dir.appendingPathComponent("file.txt"))
    return (PlaceBoundary.canonical(dir.path), dir)
}

private func writeCall(_ path: String, _ content: String) -> ToolCall {
    ToolCall(name: "write_file", arguments: .object(["path": .string(path), "content": .string(content)]))
}

@Test func declinedWriteLeavesFileUntouchedAndReturnsARefusal() async throws {
    let (place, dir) = try consentPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await HarnessActTools.execute(
        writeCall("file.txt", "overwrite\n"),
        place: place,
        context: HarnessActContext(sessionID: "s", ask: alwaysDeclineConsent, state: HarnessActState()))
    #expect(result.contains("declined"))
    #expect((try? String(contentsOfFile: place + "/file.txt")) == "keep\n")
}

@Test func dontAskGrantCoversOnlyItsExactTool() async throws {
    let (place, dir) = try consentPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let state = HarnessActState()
    let asks = Counter()
    let ask: ConsentAsk = { request in
        await asks.bump()
        return .approved(dontAskAgain: request.toolName == "write_file")
    }
    let ctx = HarnessActContext(sessionID: "s", ask: ask, state: state)

    _ = await HarnessActTools.execute(writeCall("a.txt", "1\n"), place: place, context: ctx)
    _ = await HarnessActTools.execute(writeCall("b.txt", "2\n"), place: place, context: ctx)
    _ = await HarnessActTools.execute(writeCall("c.txt", "3\n"), place: place, context: ctx)
    #expect(await asks.value == 1)

    let commandAsks = Counter()
    let cmdAsk: ConsentAsk = { _ in await commandAsks.bump(); return .declined }
    let cmdCtx = HarnessActContext(sessionID: "s", ask: cmdAsk, state: state)
    _ = await HarnessActTools.execute(
        ToolCall(name: "run_command", arguments: .object(["argv": .array([.string("/bin/echo"), .string("x")])])),
        place: place, context: cmdCtx)
    #expect(await commandAsks.value == 1)
}

@Test func grantIsScopedPerSessionAndDoesNotLeak() async throws {
    let (place, dir) = try consentPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let state = HarnessActState()
    let asks = Counter()
    let ask: ConsentAsk = { _ in await asks.bump(); return .approved(dontAskAgain: true) }

    _ = await HarnessActTools.execute(
        writeCall("a.txt", "1\n"), place: place,
        context: HarnessActContext(sessionID: "one", ask: ask, state: state))
    _ = await HarnessActTools.execute(
        writeCall("b.txt", "2\n"), place: place,
        context: HarnessActContext(sessionID: "two", ask: ask, state: state))
    #expect(await asks.value == 2)
}

private actor Counter {
    private(set) var value = 0
    func bump() { value += 1 }
}
