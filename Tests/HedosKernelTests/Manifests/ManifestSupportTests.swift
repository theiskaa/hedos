import Foundation
import Testing

@testable import HedosKernel

private func supportRecord(in dir: URL) throws -> ModelRecord {
    let bundle = dir.appendingPathComponent("model", isDirectory: true)
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    let weight = bundle.appendingPathComponent("weights.xyz")
    try Data("xyz".utf8).write(to: weight)
    var record = ModelRecord(
        name: "model", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    record.primaryWeightPath = weight.path
    return record
}

@Test func placeholderExpansionNeverRescansReplacementText() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try supportRecord(in: dir)
    let workdir = dir.appendingPathComponent("work")
    let outputs = dir.appendingPathComponent("out")
    let payload: JSONValue = .object(["prompt": .string("see {workdir} and {python}")])

    let tokens = try ManifestSupport.substituted(
        command: "run --model {model} --text {prompt}",
        record: record, payload: payload, workdir: workdir, outputs: outputs, envDir: nil)

    #expect(tokens.contains { $0.contains("{workdir}") })
    #expect(tokens.contains { $0.contains("{python}") })
    #expect(tokens.contains(SidecarModelPaths.resolve(record).snapshot))
    #expect(!tokens.contains { $0.contains(workdir.path) })
}

@Test func commandNamingPythonWithoutEnvThrowsButPromptDoesNot() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try supportRecord(in: dir)
    let workdir = dir.appendingPathComponent("work")
    let outputs = dir.appendingPathComponent("out")

    let promptPython: JSONValue = .object(["prompt": .string("use {python} please")])
    #expect(throws: Never.self) {
        _ = try ManifestSupport.substituted(
            command: "run --text {prompt}", record: record, payload: promptPython,
            workdir: workdir, outputs: outputs, envDir: nil)
    }

    #expect(throws: KernelError.self) {
        _ = try ManifestSupport.substituted(
            command: "{python} run.py", record: record, payload: .object([:]),
            workdir: workdir, outputs: outputs, envDir: nil)
    }
}

@Test func emptyCommandStillThrows() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try supportRecord(in: dir)
    #expect(throws: KernelError.self) {
        _ = try ManifestSupport.substituted(
            command: "", record: record, payload: .object([:]),
            workdir: dir, outputs: dir, envDir: nil)
    }
}

@Test func conversationTextRendersEveryTurnInOrder() {
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("system"), "content": .string("be kind")]),
            .object(["role": .string("user"), "content": .string("hi")]),
            .object(["role": .string("assistant"), "content": .string("hello")]),
            .object(["role": .string("user"), "content": .string("bye")]),
        ])
    ])
    #expect(
        ManifestSupport.promptText(from: payload)
            == "system: be kind\nuser: hi\nassistant: hello\nuser: bye")
}

@Test func explicitPromptWinsOverMessages() {
    let payload: JSONValue = .object([
        "prompt": .string("just this"),
        "messages": .array([
            .object(["role": .string("user"), "content": .string("not this")])
        ]),
    ])
    #expect(ManifestSupport.promptText(from: payload) == "just this")
}
