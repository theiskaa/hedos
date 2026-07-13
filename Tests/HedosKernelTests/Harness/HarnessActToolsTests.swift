import Foundation
import Testing

@testable import HedosKernel

private func actPlace() throws -> (place: String, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    try Data("original\ncontent\n".utf8)
        .write(to: dir.appendingPathComponent("existing.txt"))
    return (PlaceBoundary.canonical(dir.path), dir)
}

private let approveAll: ConsentAsk = { _ in .approved(dontAskAgain: false) }

private func actCall(_ name: String, _ arguments: [String: JSONValue]) -> ToolCall {
    ToolCall(name: name, arguments: .object(arguments))
}

private func context(ask: @escaping ConsentAsk = approveAll, session: String = "s")
    -> HarnessActContext
{
    HarnessActContext(sessionID: session, ask: ask, state: HarnessActState())
}

@Test func writeFileCreatesInsidePlaceWhenApproved() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await HarnessActTools.execute(
        actCall("write_file", ["path": .string("sub/new.txt"), "content": .string("hi\n")]),
        place: place, context: context())
    #expect(result.contains("wrote sub/new.txt"))
    #expect(FileManager.default.fileExists(atPath: place + "/sub/new.txt"))
    #expect((try? String(contentsOfFile: place + "/sub/new.txt")) == "hi\n")
}

@Test func writeFileRefusesEscapesBeforeAsking() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let asked = Recorder()
    for attack in ["../escape.txt", "sub/../../escape.txt", "/tmp/hedos-escape.txt"] {
        let result = await HarnessActTools.execute(
            actCall("write_file", ["path": .string(attack), "content": .string("x")]),
            place: place, context: context(ask: { req in await asked.record(req); return .approved(dontAskAgain: false) }))
        #expect(!result.contains("wrote"))
    }
    #expect(await asked.count == 0)
    #expect(!FileManager.default.fileExists(atPath: "/tmp/hedos-escape.txt"))
}

@Test func writeFileEnforcesTheCap() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let big = String(repeating: "a", count: HarnessActTools.writeCapBytes + 1)
    let result = await HarnessActTools.execute(
        actCall("write_file", ["path": .string("big.txt"), "content": .string(big)]),
        place: place, context: context())
    #expect(result.contains("cap"))
    #expect(!FileManager.default.fileExists(atPath: place + "/big.txt"))
}

@Test func writeFlagsForeignFileOverwrite() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let seen = ConsentBox()
    _ = await HarnessActTools.execute(
        actCall("write_file", ["path": .string("existing.txt"), "content": .string("new\n")]),
        place: place, context: context(ask: { req in await seen.set(req); return .approved(dontAskAgain: false) }))
    let request = await seen.request
    if case .write(_, _, let foreign)? = request?.kind {
        #expect(foreign != nil)
        #expect(foreign?.contains("existing.txt") == true)
    } else {
        Issue.record("expected a write consent request")
    }
}

@Test func writeDoesNotFlagFilesTheModelCreated() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let state = HarnessActState()
    let ctx = HarnessActContext(sessionID: "s", ask: approveAll, state: state)
    _ = await HarnessActTools.execute(
        actCall("write_file", ["path": .string("mine.txt"), "content": .string("v1\n")]),
        place: place, context: ctx)
    let seen = ConsentBox()
    _ = await HarnessActTools.execute(
        actCall("write_file", ["path": .string("mine.txt"), "content": .string("v2\n")]),
        place: place,
        context: HarnessActContext(
            sessionID: "s", ask: { req in await seen.set(req); return .approved(dontAskAgain: false) },
            state: state))
    if case .write(_, _, let foreign)? = await seen.request?.kind {
        #expect(foreign == nil)
    } else {
        Issue.record("expected a write consent request")
    }
}

@Test func editFileReplacesUniqueString() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await HarnessActTools.execute(
        actCall(
            "edit_file",
            ["path": .string("existing.txt"), "old": .string("original"), "new": .string("changed")]),
        place: place, context: context())
    #expect(result.contains("edited existing.txt"))
    #expect((try? String(contentsOfFile: place + "/existing.txt")) == "changed\ncontent\n")
}

@Test func editFileFailsOnNonUniqueOldAsANormalResult() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data("dup\ndup\n".utf8).write(to: dir.appendingPathComponent("dup.txt"))
    let result = await HarnessActTools.execute(
        actCall("edit_file", ["path": .string("dup.txt"), "old": .string("dup"), "new": .string("x")]),
        place: place, context: context())
    #expect(result.contains("2 times"))
    #expect((try? String(contentsOfFile: place + "/dup.txt")) == "dup\ndup\n")
}

@Test func editFileFailsOnMissingOldAsANormalResult() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await HarnessActTools.execute(
        actCall(
            "edit_file",
            ["path": .string("existing.txt"), "old": .string("absent"), "new": .string("x")]),
        place: place, context: context())
    #expect(result.contains("not found"))
    #expect((try? String(contentsOfFile: place + "/existing.txt")) == "original\ncontent\n")
}

@Test func editConsentRequestCarriesTheEditToolNameNotWrite() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let seen = ConsentBox()
    _ = await HarnessActTools.execute(
        actCall(
            "edit_file",
            ["path": .string("existing.txt"), "old": .string("original"), "new": .string("x")]),
        place: place, context: context(ask: { req in await seen.set(req); return .declined }))
    #expect(await seen.request?.toolName == "edit_file")
}

@Test func writeCancelledDuringConsentLeavesTheFileUntouched() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let result = await Task {
        await HarnessActTools.execute(
            actCall("write_file", ["path": .string("cancelled.txt"), "content": .string("x\n")]),
            place: place,
            context: context(ask: { _ in
                withUnsafeCurrentTask { $0?.cancel() }
                return .approved(dontAskAgain: false)
            }))
    }.value
    #expect(result.contains("cancelled"))
    #expect(!FileManager.default.fileExists(atPath: place + "/cancelled.txt"))
}

private actor Recorder {
    private(set) var count = 0
    func record(_ request: ConsentRequest) { count += 1 }
}

private actor ConsentBox {
    private(set) var request: ConsentRequest?
    func set(_ request: ConsentRequest) { self.request = request }
}

@Test func writeFileRefusesWhenTheFileChangesDuringConsent() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let target = place + "/existing.txt"
    let result = await HarnessActTools.execute(
        actCall(
            "write_file",
            ["path": .string("existing.txt"), "content": .string("replacement\n")]),
        place: place,
        context: context(ask: { _ in
            try? Data("concurrent edit\n".utf8).write(to: URL(fileURLWithPath: target))
            return .approved(dontAskAgain: false)
        }))
    #expect(result.contains("changed while waiting for approval"))
    #expect((try? String(contentsOfFile: target, encoding: .utf8)) == "concurrent edit\n")
}

@Test func editFileRefusesWhenTheFileChangesDuringConsent() async throws {
    let (place, dir) = try actPlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let target = place + "/existing.txt"
    let result = await HarnessActTools.execute(
        actCall(
            "edit_file",
            ["path": .string("existing.txt"), "old": .string("original"), "new": .string("updated")]),
        place: place,
        context: context(ask: { _ in
            try? Data("original\ncontent\nplus a concurrent line\n".utf8)
                .write(to: URL(fileURLWithPath: target))
            return .approved(dontAskAgain: false)
        }))
    #expect(result.contains("changed while waiting for approval"))
    #expect(
        (try? String(contentsOfFile: target, encoding: .utf8))
            == "original\ncontent\nplus a concurrent line\n")
}
