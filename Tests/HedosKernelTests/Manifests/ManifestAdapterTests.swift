import Foundation
import Testing

@testable import HedosKernel

private func realPythonPath() throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = [
        "python3", "-c", "import os, sys; print(os.path.realpath(sys.executable))",
    ]
    let stdout = Pipe()
    process.standardOutput = stdout
    try process.run()
    let data = try stdout.fileHandleForReading.readToEnd() ?? Data()
    process.waitUntilExit()
    return String(decoding: data, as: UTF8.self)
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

private func invokeManifest(
    id: String = "test-cli-\(UUID().uuidString.prefix(8))",
    command: String,
    network: Bool = false,
    execution: ExecutionMode = .stream,
    detect: ManifestDetect? = ManifestDetect(fileExtension: "xyz")
) -> RuntimeManifest {
    RuntimeManifest(
        id: id,
        modalities: [.text],
        capabilities: [.chat, .complete],
        execution: execution,
        alternatives: [],
        detect: detect,
        env: nil,
        serve: nil,
        invoke: ManifestInvoke(command: command),
        permissions: ManifestPermissions(network: network, paths: ["{model}", "{workdir}"]),
        directory: nil)
}

private func xyzRecord(in dir: URL) throws -> ModelRecord {
    let weight = dir.appendingPathComponent("weights.xyz")
    try Data("xyz".utf8).write(to: weight)
    var record = ModelRecord(
        name: "dark-model", modality: .unknown, capabilities: [],
        source: ModelSource(kind: SourceKind(rawValue: "fixture"), path: dir.path))
    record.primaryWeightPath = weight.path
    return record
}

private func pinned(_ record: ModelRecord, to id: String) -> ModelRecord {
    var pinnedRecord = record
    pinnedRecord.runtime = RuntimeRef(id: id, resolved: .auto, tier: .managed)
    return pinnedRecord
}

@Test func bidRequiresDetectMatchAndConsent() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let identified = Identification.identify(record)

    let matching = ManifestCommandAdapter(
        manifest: invokeManifest(command: "echo hi"), approvedNetwork: false)
    #expect(matching.bid(record, identified)?.tier == .managed)
    #expect(matching.bid(record, identified)?.preference == 100)

    let wrongDetect = ManifestCommandAdapter(
        manifest: invokeManifest(command: "echo hi", detect: ManifestDetect(fileExtension: "gguf")),
        approvedNetwork: false)
    #expect(wrongDetect.bid(record, identified) == nil)

    let unapprovedNetwork = ManifestCommandAdapter(
        manifest: invokeManifest(command: "echo hi", network: true), approvedNetwork: false)
    #expect(unapprovedNetwork.bid(record, identified) == nil)

    let approvedNetwork = ManifestCommandAdapter(
        manifest: invokeManifest(command: "echo hi", network: true), approvedNetwork: true)
    #expect(approvedNetwork.bid(record, identified) != nil)
}

@Test func commandTemplateSubstitutesPerToken() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let workdir = dir.appendingPathComponent("work")
    let outputs = dir.appendingPathComponent("out")
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("hello wide world")])
        ])
    ])

    let tokens = try ManifestSupport.substituted(
        command: "run --model {model} --prompt {prompt} --out {outputs}",
        record: record, payload: payload, workdir: workdir, outputs: outputs, envDir: nil)
    #expect(tokens.count == 7)
    #expect(tokens[4] == "hello wide world")
    #expect(tokens[6] == outputs.path)

    #expect(throws: KernelError.self) {
        _ = try ManifestSupport.substituted(
            command: "{python} run.py", record: record, payload: payload,
            workdir: workdir, outputs: outputs, envDir: nil)
    }
}

@Test func commandAdapterStreamsStdoutViaFakeCommand() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let script = dir.appendingPathComponent("fake_cli.py")
    try Data(
        """
        import sys
        print("echo:", sys.argv[1])
        """.utf8
    ).write(to: script)

    let manifest = invokeManifest(command: "\(try realPythonPath()) \(script.path) {prompt}")
    defer {
        try? FileManager.default.removeItem(at: Registry.defaultDirectory()
            .appendingPathComponent("workdirs/\(ManifestSupport.slug(manifest.id))"))
    }
    let adapter = ManifestCommandAdapter(manifest: manifest, approvedNetwork: false)
    let payload: JSONValue = .object([
        "messages": .array([
            .object(["role": .string("user"), "content": .string("ping")])
        ])
    ])

    var text = ""
    var sawDone = false
    for try await chunk in adapter.invoke(pinned(record, to: manifest.id), .chat, payload: payload)
    {
        if case .text(let delta) = chunk { text += delta }
        if case .done = chunk { sawDone = true }
    }
    #expect(text.contains("echo: ping"))
    #expect(sawDone)
}

@Test func commandAdapterJobCollectsOutputs() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let script = dir.appendingPathComponent("fake_job.py")
    try Data(
        """
        import sys, os
        with open(os.path.join(sys.argv[1], "result.png"), "wb") as f:
            f.write(b"PNGDATA")
        """.utf8
    ).write(to: script)

    let manifest = invokeManifest(
        command: "\(try realPythonPath()) \(script.path) {outputs}", execution: .job)
    defer {
        try? FileManager.default.removeItem(at: Registry.defaultDirectory()
            .appendingPathComponent("workdirs/\(ManifestSupport.slug(manifest.id))"))
    }
    let adapter = ManifestCommandAdapter(manifest: manifest, approvedNetwork: false)

    var results: [(Data, String)] = []
    for try await event in adapter.run(
        pinned(record, to: manifest.id), .chat, payload: .object([:]))
    {
        if case .result(let data, let ext) = event { results.append((data, ext)) }
    }
    #expect(results.count == 1)
    #expect(results.first?.0 == Data("PNGDATA".utf8))
    #expect(results.first?.1 == "png")
}

@Test func commandAdapterNonzeroExitFailsWithStderr() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let script = dir.appendingPathComponent("fake_fail.py")
    try Data(
        """
        import sys
        print("the tool exploded", file=sys.stderr)
        sys.exit(3)
        """.utf8
    ).write(to: script)

    let manifest = invokeManifest(command: "\(try realPythonPath()) \(script.path)")
    defer {
        try? FileManager.default.removeItem(at: Registry.defaultDirectory()
            .appendingPathComponent("workdirs/\(ManifestSupport.slug(manifest.id))"))
    }
    let adapter = ManifestCommandAdapter(manifest: manifest, approvedNetwork: false)

    do {
        for try await _ in adapter.invoke(
            pinned(record, to: manifest.id), .chat, payload: .object([:])) {}
        Issue.record("expected the command failure to surface")
    } catch {
        #expect(String(describing: error).contains("the tool exploded"))
    }
}

@Test func unapprovedInvokeRefusesWithConsentHint() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try xyzRecord(in: dir)
    let manifest = invokeManifest(command: "echo hi", network: true)
    let adapter = ManifestCommandAdapter(manifest: manifest, approvedNetwork: false)

    do {
        for try await _ in adapter.invoke(
            pinned(record, to: manifest.id), .chat, payload: .object([:])) {}
        Issue.record("expected the consent refusal")
    } catch {
        #expect(String(describing: error).contains("network permission"))
    }
}

@Test func genericProfilesGateNetworkByVariant() throws {
    let generic = try #require(RuntimeBundle.directory(named: "generic"))
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let probe = """
        import socket
        print("SANDBOX-ALIVE", flush=True)
        try:
            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            client = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            client.settimeout(3)
            client.connect(server.getsockname())
            print("NETWORK-ALLOWED", flush=True)
        except OSError:
            print("NETWORK-DENIED", flush=True)
        """

    func run(profile: String) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/sandbox-exec")
        process.arguments = [
            "-f", generic.appendingPathComponent(profile).path,
            "-D", "VENV=\(ManifestSupport.canonicalPath(dir))",
            "-D", "UVPY=\(ManifestSupport.canonicalPath(dir))",
            "-D", "MODEL=\(ManifestSupport.canonicalPath(dir))",
            "-D", "WORKDIR=\(ManifestSupport.canonicalPath(dir))",
            "-D", "RESOURCES=\(generic.path)",
            "-D", "TMP=\(ManifestSupport.canonicalPath(FileManager.default.temporaryDirectory))",
            "-D",
            "CACHE=\(URL(fileURLWithPath: ManifestSupport.canonicalPath(FileManager.default.temporaryDirectory)).deletingLastPathComponent().appendingPathComponent("C").path)",
            try realPythonPath(), "-c", probe,
        ]
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "PYTHONPATH")
        environment.removeValue(forKey: "PYTHONHOME")
        process.environment = environment
        process.currentDirectoryURL = dir
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr
        try process.run()
        let output = String(
            decoding: try stdout.fileHandleForReading.readToEnd() ?? Data(), as: UTF8.self)
        let diagnostics = String(
            decoding: try stderr.fileHandleForReading.readToEnd() ?? Data(), as: UTF8.self)
        process.waitUntilExit()
        _ = diagnostics
        return output
    }

    let denied = try run(profile: "generic-net-off.sb")
    #expect(denied.contains("SANDBOX-ALIVE"))
    #expect(denied.contains("NETWORK-DENIED"))

    let allowed = try run(profile: "generic-net-on.sb")
    #expect(allowed.contains("SANDBOX-ALIVE"))
    #expect(!allowed.contains("NETWORK-DENIED"))
}
