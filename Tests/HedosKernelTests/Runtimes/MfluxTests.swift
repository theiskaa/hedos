import Foundation
import Testing

@testable import HedosKernel

private func imageSidecarSpec() -> SidecarSpec {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
    return SidecarSpec(
        runtimeID: "fake-image-\(UUID().uuidString)",
        executable: URL(fileURLWithPath: "/usr/bin/env"),
        arguments: ["python3", script.path, "normal"],
        readyTimeout: .seconds(15))
}

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

private func imagePayload(steps: Int, prompt: String = "a lighthouse at dusk", seed: Int? = nil)
    -> JSONValue
{
    var fields: [String: JSONValue] = [
        "prompt": .string(prompt),
        "steps": .int(steps),
        "guidance": .double(0.0),
        "size": .string("1024x1024"),
    ]
    if let seed { fields["seed"] = .int(seed) }
    return .object(fields)
}

private struct SidecarImageJobAdapter: RuntimeAdapter, JobRunning {
    let supervisor: SidecarSupervisor
    let spec: SidecarSpec

    var id: RuntimeID { "python:mflux" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        let supervisor = supervisor
        let spec = spec
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    continuation.yield(.status("Preparing image runtime…"))
                    try await supervisor.ensureRunning(spec)
                    var control: [String: JSONValue] = ["op": .string("image")]
                    if case .object(let fields) = payload {
                        for (key, value) in fields { control[key] = value }
                    }
                    let stream = await supervisor.jobRequest(spec, .object(control))
                    for try await event in stream {
                        continuation.yield(event)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

@Test func mfluxAdapterBidMatrix() {
    let adapter = MfluxAdapter()
    let fluxImage = IdentifiedModel(
        format: .diffusers, modality: .image, capabilities: [.image], execution: .job,
        params: PipelineFamilyRegistry.fluxParams, pipelineClass: "FluxPipeline")
    let diffusersWithoutCapability = IdentifiedModel(
        format: .diffusers, modality: .image, capabilities: [], execution: .job,
        pipelineClass: "FluxPipeline")
    let nonFluxImage = IdentifiedModel(
        format: .diffusers, modality: .image, capabilities: [.image], execution: .job,
        params: PipelineFamilyRegistry.fluxParams, pipelineClass: "StableDiffusionPipeline")
    let classlessDiffusers = IdentifiedModel(
        format: .diffusers, modality: .image, capabilities: [.image], execution: .job,
        params: PipelineFamilyRegistry.fluxParams)
    let speechMlx = IdentifiedModel(
        format: .safetensors, modality: .speech, capabilities: [.speak], execution: .stream)
    let textGguf = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat], execution: .stream)

    let record = Fixtures.flux()
    let bid = adapter.bid(record, fluxImage)
    #expect(bid?.tier == .managed)
    #expect(bid?.alternatives == ["python:diffusers"])
    #expect(adapter.bid(record, diffusersWithoutCapability) == nil)
    #expect(adapter.bid(record, nonFluxImage) == nil)
    #expect(adapter.bid(record, classlessDiffusers) == nil)
    #expect(adapter.bid(record, speechMlx) == nil)
    #expect(adapter.bid(record, textGguf) == nil)
}

@Test func mfluxRuntimeBundleShipsCompleteAndValid() throws {
    let bundle = try #require(RuntimeBundle.directory(named: "python-mflux"))
    let fm = FileManager.default
    for file in ["main.py", "manifest.toml", "requirements.in", "requirements.lock", "sandbox.sb"]
    {
        #expect(
            fm.fileExists(atPath: bundle.appendingPathComponent(file).path),
            "missing \(file)")
    }
    let manifest = try String(
        contentsOf: bundle.appendingPathComponent("manifest.toml"), encoding: .utf8)
    #expect(manifest.contains("python:mflux"))
    #expect(manifest.contains(#"execution    = "job""#))
    #expect(manifest.contains(#"alternatives = ["python:diffusers"]"#))
    #expect(manifest.contains("network = false"))
    let profile = try String(
        contentsOf: bundle.appendingPathComponent("sandbox.sb"), encoding: .utf8)
    #expect(profile.contains("(deny network*)"))
    #expect(profile.contains("system.sb"))
    let lock = try String(
        contentsOf: bundle.appendingPathComponent("requirements.lock"), encoding: .utf8)
    #expect(lock.contains("mflux=="))
    #expect(lock.contains("--hash=sha256:"))
    #expect(!fm.fileExists(atPath: bundle.appendingPathComponent("__pycache__").path))
}

@Test func sandboxProfileLaunchesUnderSeatbeltAndDeniesNetwork() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let fm = FileManager.default
    let envDir = dir.appendingPathComponent("env")
    try fm.createDirectory(
        at: envDir.appendingPathComponent("bin"), withIntermediateDirectories: true)
    let realPython = try realPythonPath()
    try fm.createSymbolicLink(
        at: envDir.appendingPathComponent("bin/python"),
        withDestinationURL: URL(fileURLWithPath: realPython))
    let modelDir = dir.appendingPathComponent("model")
    try fm.createDirectory(at: modelDir, withIntermediateDirectories: true)
    let bundle = try #require(RuntimeBundle.directory(named: "python-mflux"))
    let record = ModelRecord(
        name: "flux-sandbox-probe", modality: .image, capabilities: [.image],
        source: ModelSource(kind: .folder, path: modelDir.path))
    let spec = try SidecarBundle.spec(
        runtimeID: .mflux, record: record, bundle: bundle, envDir: envDir,
        workdirRoot: dir, workdirName: "workdir",
        extraArguments: ["--name", record.name])

    let pythonPath = envDir.appendingPathComponent("bin/python").path
    let pythonIndex = try #require(spec.arguments.firstIndex(of: pythonPath))
    let probe = """
        import socket
        print("SANDBOX-ALIVE", flush=True)
        try:
            connection = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            connection.settimeout(3)
            connection.connect(("1.1.1.1", 80))
            print("NETWORK-ALLOWED", flush=True)
        except OSError:
            print("NETWORK-DENIED", flush=True)
        """

    let process = Process()
    process.executableURL = spec.executable
    process.arguments = Array(spec.arguments[..<(pythonIndex + 1)]) + ["-c", probe]
    var environment = spec.environment
    environment["HOME"] = dir.path
    process.environment = environment
    process.currentDirectoryURL = spec.workingDirectory
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

    #expect(process.terminationStatus == 0, "sandboxed python failed: \(diagnostics)")
    #expect(output.contains("SANDBOX-ALIVE"), "sandboxed python never ran: \(diagnostics)")
    #expect(output.contains("NETWORK-DENIED"))
    #expect(!output.contains("NETWORK-ALLOWED"))
}

@Test func mfluxSpecDeclaresLongCancelGrace() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let modelDir = dir.appendingPathComponent("model")
    try FileManager.default.createDirectory(at: modelDir, withIntermediateDirectories: true)
    let record = ModelRecord(
        name: "flux-grace-probe", modality: .image, capabilities: [.image],
        source: ModelSource(kind: .folder, path: modelDir.path))
    let descriptor = MfluxAdapter.descriptor(environments: .shared, workdirRoot: dir)
    let spec = try descriptor.makeSpec(record, dir.appendingPathComponent("env"))
    #expect(spec.cancelGraceTimeout == .seconds(60))
}

@Test func resolvesHFCacheFluxPipelineToMfluxManagedWithSchema() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let hubRoot = dir.appendingPathComponent("hub")
    try DiscoveryFixtures.makeHFRepo(
        at: hubRoot,
        DiscoveryFixtures.HFRepo(
            org: "black-forest-labs", repo: "FLUX.1-dev",
            files: [("transformer.safetensors", 8192)],
            modelIndexJSON: DiscoveryFixtures.fluxModelIndex,
            transformerConfigJSON: DiscoveryFixtures.fluxDevTransformerConfig))

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = ModelRecord(
        name: "black-forest-labs/FLUX.1-dev",
        modality: .unknown,
        capabilities: [],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: hubRoot.appendingPathComponent("models--black-forest-labs--FLUX.1-dev")
                .path,
            repo: "black-forest-labs/FLUX.1-dev",
            ref: "abc123def456"))
    try await registry.register(record)

    let engine = ResolutionEngine(adapters: [LlamaCppAdapter(), OllamaAdapter(), MfluxAdapter()])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "python:mflux")
    #expect(resolved.runtime.tier == .managed)
    #expect(resolved.runtime.alternatives == ["python:diffusers"])
    #expect(resolved.state == .ready)
    #expect(resolved.modality == .image)
    #expect(resolved.capabilities == [.image])
    #expect(resolved.execution == .job)

    let params = Dictionary(uniqueKeysWithValues: resolved.params.map { ($0.key, $0) })
    let steps = try #require(params["steps"])
    #expect(steps.type == .int)
    #expect(steps.defaultValue == .int(4))
    #expect(steps.range == [.int(1), .int(50)])
    let guidance = try #require(params["guidance"])
    #expect(guidance.type == .float)
    #expect(guidance.defaultValue == .double(4.0))
    #expect(guidance.range == [.double(0), .double(10)])
    let size = try #require(params["size"])
    #expect(size.type == .enumeration)
    #expect(size.values == ["512x512", "768x768", "1024x1024"])
    #expect(size.defaultValue == .string("1024x1024"))
    let seed = try #require(params["seed"])
    #expect(seed.type == .int)
    #expect(seed.defaultValue == nil)
    #expect(params["negative_prompt"] == nil)
}

@Test func devFluxKeepsGuidanceKnob() {
    let dir = try! Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let container = dir.appendingPathComponent("flux-dev")
    let transformer = container.appendingPathComponent("transformer")
    try! FileManager.default.createDirectory(at: transformer, withIntermediateDirectories: true)
    try! Data(DiscoveryFixtures.fluxModelIndex.utf8)
        .write(to: container.appendingPathComponent("model_index.json"))
    try! Data(DiscoveryFixtures.fluxDevTransformerConfig.utf8)
        .write(to: transformer.appendingPathComponent("config.json"))

    let record = ModelRecord(
        name: "flux-dev", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: container.path))
    let identified = Identification.identify(record)
    #expect(identified.pipelineClass == "FluxPipeline")
    #expect(identified.params.contains { $0.key == "guidance" })
}

@Test func schnellFluxResolvesWithoutGuidanceKnob() {
    let dir = try! Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    let schnell = dir.appendingPathComponent("flux-schnell")
    let schnellTransformer = schnell.appendingPathComponent("transformer")
    try! FileManager.default.createDirectory(
        at: schnellTransformer, withIntermediateDirectories: true)
    try! Data(DiscoveryFixtures.fluxModelIndex.utf8)
        .write(to: schnell.appendingPathComponent("model_index.json"))
    try! Data(DiscoveryFixtures.fluxSchnellTransformerConfig.utf8)
        .write(to: schnellTransformer.appendingPathComponent("config.json"))
    let schnellRecord = ModelRecord(
        name: "flux-schnell", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: schnell.path))
    let schnellIdentified = Identification.identify(schnellRecord)
    #expect(schnellIdentified.pipelineClass == "FluxPipeline")
    #expect(!schnellIdentified.params.contains { $0.key == "guidance" })
    #expect(schnellIdentified.params.contains { $0.key == "steps" })
    #expect(schnellIdentified.params.contains { $0.key == "size" })
    #expect(schnellIdentified.params.contains { $0.key == "seed" })

    let missing = dir.appendingPathComponent("flux-missing-config")
    try! FileManager.default.createDirectory(at: missing, withIntermediateDirectories: true)
    try! Data(DiscoveryFixtures.fluxModelIndex.utf8)
        .write(to: missing.appendingPathComponent("model_index.json"))
    let missingRecord = ModelRecord(
        name: "flux-missing", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: missing.path))
    let missingIdentified = Identification.identify(missingRecord)
    #expect(!missingIdentified.params.contains { $0.key == "guidance" })
}

@Test func modelIndexWithoutPipelineClassStaysRecipeNeeded() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bundle = dir.appendingPathComponent("mystery-pipeline")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data("{}".utf8).write(to: bundle.appendingPathComponent("model_index.json"))

    let record = ModelRecord(
        name: "mystery-pipeline", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    let identified = Identification.identify(record)
    #expect(identified.format == .diffusers)
    #expect(identified.capabilities.isEmpty)
    #expect(identified.params.isEmpty)

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(record)
    try await ResolutionEngine(adapters: [MfluxAdapter()]).resolveAll(in: registry)
    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.tier == .recipeNeeded)
    #expect(resolved.runtime.id == nil)
}

@Test func nonFluxDiffusersPipelinesIdentifyHonestlyButStayRecipeNeeded() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let adapter = MfluxAdapter()
    let expectations: [(pipelineClass: String, modality: Modality, capabilities: [Capability], params: [ParamSpec])] = [
        ("StableDiffusionPipeline", .image, [.image], PipelineFamilyRegistry.sd1Params),
        ("AudioLDM2Pipeline", .audio, [], []),
        ("KandinskyV22Pipeline", .image, [.image], PipelineFamilyRegistry.kandinskyParams),
        ("TextToVideoSDPipeline", .video, [], []),
    ]
    for expected in expectations {
        let bundle = dir.appendingPathComponent(expected.pipelineClass)
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        try Data("{\"_class_name\": \"\(expected.pipelineClass)\"}".utf8)
            .write(to: bundle.appendingPathComponent("model_index.json"))
        let record = ModelRecord(
            name: expected.pipelineClass, modality: .unknown, capabilities: [],
            source: ModelSource(kind: .folder, path: bundle.path))
        let identified = Identification.identify(record)
        #expect(identified.format == .diffusers)
        #expect(identified.modality == expected.modality)
        #expect(identified.capabilities == expected.capabilities)
        #expect(identified.params == expected.params)
        #expect(identified.pipelineClass == expected.pipelineClass)
        #expect(adapter.bid(record, identified) == nil)
    }

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    let record = ModelRecord(
        name: "sd-15", modality: .unknown, capabilities: [],
        source: ModelSource(
            kind: .folder, path: dir.appendingPathComponent("StableDiffusionPipeline").path))
    try await registry.register(record)
    try await ResolutionEngine(adapters: [MfluxAdapter()]).resolveAll(in: registry)
    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.tier == .recipeNeeded)
    #expect(resolved.runtime.id == nil)
    #expect(resolved.modality == .image)
    #expect(resolved.capabilities == [.image])
    #expect(!resolved.params.isEmpty)
}

@Test func supervisorStreamsImageJobFromFakeSidecar() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var events: [JobRuntimeEvent] = []
    let stream = await supervisor.jobRequest(
        spec,
        .object([
            "op": .string("image"), "prompt": .string("a lighthouse at dusk"),
            "steps": .int(3), "seed": .int(7),
        ]))
    for try await event in stream {
        events.append(event)
    }

    #expect(events.first == .started)
    let progress = events.compactMap { event -> (Int, Int)? in
        if case .progress(let step, let totalSteps) = event { return (step, totalSteps) }
        return nil
    }
    #expect(progress.map(\.0) == [1, 2, 3])
    #expect(progress.allSatisfy { $0.1 == 3 })
    let previews = events.compactMap { event -> Data? in
        if case .preview(let data) = event { return data }
        return nil
    }
    #expect(previews == [Data([0x89]) + Data("PNG-preview".utf8)])
    let results = events.compactMap { event -> (Data, String)? in
        if case .result(let data, let fileExtension) = event { return (data, fileExtension) }
        return nil
    }
    #expect(results.count == 1)
    #expect(results.first?.0 == Data([0x89]) + Data("PNG7".utf8))
    #expect(results.first?.1 == "png")
    await supervisor.shutdownAll()
}

@Test func jobRequestSurfacesSidecarError() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let stream = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "prompt": .string("fail")]))
    await #expect(throws: KernelError.self) {
        for try await _ in stream {}
    }
    #expect(await supervisor.isRunning(spec.runtimeID))
    await supervisor.shutdownAll()
}

@Test func sidecarCancelledEventDoesNotReadAsSuccessfulCompletion() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let stream = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "prompt": .string("abort")]))
    await #expect(throws: CancellationError.self) {
        for try await _ in stream {}
    }
    #expect(await supervisor.isRunning(spec.runtimeID))
    await supervisor.shutdownAll()
}

@Test func cancellingJobWaitingForBusySidecarDoesNotDisturbRunningJob() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    let (progress, progressContinuation) = AsyncStream<Int>.makeStream()
    let firstTask = Task<[JobRuntimeEvent], Error> {
        var events: [JobRuntimeEvent] = []
        let stream = await supervisor.jobRequest(
            spec, .object(["op": .string("image"), "steps": .int(30), "seed": .int(1)]))
        for try await event in stream {
            events.append(event)
            if case .progress(let step, _) = event {
                progressContinuation.yield(step)
            }
        }
        progressContinuation.finish()
        return events
    }
    var progressIterator = progress.makeAsyncIterator()
    while let step = await progressIterator.next(), step < 2 {}

    let secondTask = Task<Bool, Error> {
        let stream = await supervisor.jobRequest(
            spec, .object(["op": .string("image"), "steps": .int(2), "seed": .int(5)]))
        var sawEvent = false
        for try await _ in stream {
            sawEvent = true
        }
        return sawEvent
    }
    try await Task.sleep(for: .milliseconds(100))
    secondTask.cancel()

    let firstEvents = try await firstTask.value
    #expect(
        firstEvents.contains { event in
            if case .result(let data, _) = event {
                return data == Data([0x89]) + Data("PNG1".utf8)
            }
            return false
        })
    let secondSawEvent = (try? await secondTask.value) ?? false
    #expect(!secondSawEvent)
    #expect(await supervisor.isRunning(spec.runtimeID))

    var thirdEvents: [JobRuntimeEvent] = []
    let third = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "steps": .int(2), "seed": .int(9)]))
    for try await event in third {
        thirdEvents.append(event)
    }
    #expect(
        thirdEvents.contains { event in
            if case .result(let data, _) = event {
                return data == Data([0x89]) + Data("PNG9".utf8)
            }
            return false
        })
    await supervisor.shutdownAll()
}

@Test func idleTimeoutReclaimsImageSidecarObservedViaGovernorState() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    let governor = MemoryGovernor(totalMemoryMB: 262_144)
    let runtime = PythonSidecarRuntime(
        descriptor: fakeSidecarDescriptor(spec: spec, warmWindow: .milliseconds(300)),
        governor: governor, supervisor: supervisor)
    let record = Fixtures.flux()

    var sawResult = false
    for try await event in runtime.job(
        record, op: "image", payload: imagePayload(steps: 2, seed: 3))
    {
        if case .result = event { sawResult = true }
    }
    #expect(sawResult)

    #expect(await governor.isResident(record.id))
    #expect(await supervisor.isRunning(spec.runtimeID))

    var reclaimed = false
    for _ in 0..<200 {
        let resident = await governor.isResident(record.id)
        let running = await supervisor.isRunning(spec.runtimeID)
        if !resident && !running {
            reclaimed = true
            break
        }
        try await Task.sleep(for: .milliseconds(50))
    }
    #expect(reclaimed)
}

@Test func cancelDrainsDiffusionAndKeepsSidecarWarm() async throws {
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    try await supervisor.ensureRunning(spec)

    var sawResult = false
    let stream = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "steps": .int(50), "seed": .int(1)]))
    for try await event in stream {
        if case .progress(let step, _) = event, step >= 2 { break }
        if case .result = event { sawResult = true }
    }
    #expect(!sawResult)
    #expect(await supervisor.isRunning(spec.runtimeID))

    var events: [JobRuntimeEvent] = []
    let second = await supervisor.jobRequest(
        spec, .object(["op": .string("image"), "steps": .int(2), "seed": .int(9)]))
    for try await event in second {
        events.append(event)
    }
    #expect(
        events.contains { event in
            if case .result(let data, _) = event {
                return data == Data([0x89]) + Data("PNG9".utf8)
            }
            return false
        })
    #expect(await supervisor.isRunning(spec.runtimeID))
    await supervisor.shutdownAll()
}

@Test func imageJobLandsPNGArtifactWithFullProvenance() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    let kernel = Kernel(
        directory: dir,
        adapters: [SidecarImageJobAdapter(supervisor: supervisor, spec: spec)])
    let record = Fixtures.flux()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(
        record.id, .image, payload: imagePayload(steps: 3, seed: 7))

    var events: [JobEvent] = []
    for await event in await kernel.jobEvents(id: jobID) {
        events.append(event)
    }

    #expect(events.contains(.preparing))
    #expect(events.contains(.status("Preparing image runtime…")))
    #expect(events.contains(.running))
    let preparingIndex = try #require(events.firstIndex(of: .preparing))
    let runningIndex = try #require(events.firstIndex(of: .running))
    #expect(preparingIndex < runningIndex)
    let fractions = events.compactMap { event -> Double? in
        if case .progress(let progress) = event { return progress.fraction }
        return nil
    }
    let expectedFractions: [Double] = [1.0 / 3.0, 2.0 / 3.0, 1.0]
    #expect(fractions == expectedFractions)
    #expect(
        events.contains {
            if case .preview = $0 { return true }
            return false
        })

    let job = try #require(try await kernel.job(id: jobID))
    #expect(job.state == .done)
    #expect(job.result.count == 1)
    #expect(events.last == .done(result: job.result))

    let artifact = try #require(try await kernel.artifactStore.get(id: job.result[0]))
    #expect(artifact.model == "FLUX.1-schnell")
    #expect(artifact.modelID == record.id)
    #expect(artifact.runtime == "python:mflux")
    #expect(artifact.capability == .image)
    #expect(artifact.jobID == jobID)
    #expect(artifact.durationMs >= 0)
    #expect(artifact.path.hasSuffix(".png"))
    let params = try #require(artifact.params.objectValue)
    #expect(params["prompt"] == .string("a lighthouse at dusk"))
    #expect(params["steps"] == .int(3))
    #expect(params["seed"] == .int(7))
    let bytes = try Data(
        contentsOf: dir.appendingPathComponent("outputs").appendingPathComponent(artifact.path))
    #expect(bytes == Data([0x89]) + Data("PNG7".utf8))
    await supervisor.shutdownAll()
}

@Test func cancelMidDiffusionUnwindsJobAndNextSubmitReusesSidecar() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let spec = imageSidecarSpec()
    let supervisor = SidecarSupervisor()
    let kernel = Kernel(
        directory: dir,
        adapters: [SidecarImageJobAdapter(supervisor: supervisor, spec: spec)])
    let record = Fixtures.flux()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(
        record.id, .image, payload: imagePayload(steps: 50, seed: 1))
    var events: [JobEvent] = []
    for await event in await kernel.jobEvents(id: jobID) {
        events.append(event)
        if case .progress(let progress) = event, progress.step == 2 {
            await kernel.cancel(jobID: jobID)
        }
    }

    #expect(events.last == .cancelled)
    let cancelled = try #require(try await kernel.job(id: jobID))
    #expect(cancelled.state == .cancelled)
    #expect(cancelled.result.isEmpty)
    #expect(try await kernel.artifactStore.list().isEmpty)
    #expect(await supervisor.isRunning(spec.runtimeID))

    let secondID = try await kernel.submit(
        record.id, .image, payload: imagePayload(steps: 2, seed: 9))
    for await _ in await kernel.jobEvents(id: secondID) {}
    let second = try #require(try await kernel.job(id: secondID))
    #expect(second.state == .done)
    #expect(second.result.count == 1)
    await supervisor.shutdownAll()
}

@Test func mfluxAdapterRefusesToStreamImages() async throws {
    let adapter = MfluxAdapter()
    let stream = adapter.invoke(Fixtures.flux(), .image, payload: .null)
    await #expect(throws: KernelError.self) {
        for try await _ in stream {}
    }
}
