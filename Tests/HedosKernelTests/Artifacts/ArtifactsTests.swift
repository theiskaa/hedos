import Foundation
import Testing

@testable import HedosKernel

struct DeterministicImageAdapter: RuntimeAdapter, JobRunning {
    var emitPreview = true

    var id: RuntimeID { "fake:image" }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        nil
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(throwing: KernelError.runtimeFailed("fake adapter only runs jobs"))
        }
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        let emitPreview = emitPreview
        return AsyncThrowingStream { continuation in
            continuation.yield(.started)
            continuation.yield(.progress(step: 1, totalSteps: 2))
            if emitPreview {
                continuation.yield(.preview(Self.previewBytes(payload)))
            }
            continuation.yield(.progress(step: 2, totalSteps: 2))
            continuation.yield(.result(data: Self.imageBytes(payload), fileExtension: "png"))
            continuation.finish()
        }
    }

    static func imageBytes(_ payload: JSONValue) -> Data {
        Data("png:".utf8) + canonical(payload)
    }

    static func previewBytes(_ payload: JSONValue) -> Data {
        Data("thumb:".utf8) + canonical(payload)
    }

    private static func canonical(_ payload: JSONValue) -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return (try? encoder.encode(payload)) ?? Data()
    }
}

private func fakeImageRecord() -> ModelRecord {
    var record = Fixtures.flux()
    record.runtime = RuntimeRef(id: "fake:image", resolved: .auto, tier: .managed)
    return record
}

private func imagePayload(seed: Int? = 771_342) -> JSONValue {
    var fields: [String: JSONValue] = [
        "prompt": .string("a lighthouse at dusk"),
        "steps": .int(4),
        "guidance": .double(0.0),
        "size": .string("1024x1024"),
    ]
    if let seed {
        fields["seed"] = .int(seed)
    }
    return .object(fields)
}

private func makeKernel(_ dir: URL, emitPreview: Bool = true) -> Kernel {
    Kernel(directory: dir, adapters: [DeterministicImageAdapter(emitPreview: emitPreview)])
}

private func runToDone(_ kernel: Kernel, _ jobID: String) async throws -> Job {
    for await _ in await kernel.jobEvents(id: jobID) {}
    let job = try #require(try await kernel.job(id: jobID))
    #expect(job.state == .done)
    return job
}

private func resultArtifact(_ kernel: Kernel, of job: Job) async throws -> Artifact {
    let artifactID = try #require(job.result.first)
    return try #require(try await kernel.artifact(id: artifactID))
}

private func seedValue(_ params: JSONValue) throws -> Int {
    guard case .object(let fields) = params, case .int(let seed) = try #require(fields["seed"])
    else {
        throw KernelError.runtimeFailed("params carry no int seed")
    }
    return seed
}

@Test func completedJobWritesArtifactWithProvenanceSidecar() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)
    let payload = imagePayload()

    let jobID = try await kernel.submit(record.id, .image, payload: payload)
    let job = try await runToDone(kernel, jobID)

    let listed = try await kernel.artifacts()
    #expect(listed.count == 1)
    let artifact = try #require(listed.first)
    #expect(job.result == [artifact.id])
    #expect(artifact.model == record.name)
    #expect(artifact.modelID == record.id)
    #expect(artifact.runtime == "fake:image")
    #expect(artifact.capability == .image)
    #expect(artifact.params == payload)
    #expect(artifact.jobID == jobID)
    #expect(artifact.durationMs >= 0)
    #expect(abs(artifact.createdAt.timeIntervalSinceNow) < 60)

    let outputs = dir.appendingPathComponent("outputs")
    let year = String(Calendar(identifier: .gregorian).component(.year, from: artifact.createdAt))
    #expect(artifact.path.hasPrefix("\(year)/"))
    #expect(artifact.path.hasSuffix(".png"))
    let blob = try Data(contentsOf: outputs.appendingPathComponent(artifact.path))
    #expect(blob == DeterministicImageAdapter.imageBytes(payload))

    let sidecarURL = outputs.appendingPathComponent("\(year)/\(artifact.id).json")
    let sidecar =
        try JSONSerialization.jsonObject(with: Data(contentsOf: sidecarURL)) as! [String: Any]
    #expect(sidecar["artifact"] as? String == artifact.path)
    #expect(sidecar["model"] as? String == record.name)
    #expect(sidecar["runtime"] as? String == "fake:image")
    #expect(sidecar["capability"] as? String == "image")
    #expect(sidecar["jobID"] as? String == jobID)
    #expect(sidecar["durationMs"] as? Int != nil)
    #expect(sidecar["createdAt"] as? String != nil)
    let params = try #require(sidecar["params"] as? [String: Any])
    #expect(params["prompt"] as? String == "a lighthouse at dusk")
    #expect(params["seed"] as? Int == 771_342)
}

@Test func rerunReproducesByteIdenticalOutputAndReloadListsBoth() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let firstJob = try await kernel.submit(record.id, .image, payload: imagePayload())
    _ = try await runToDone(kernel, firstJob)
    let original = try #require(try await kernel.artifacts().first)

    let rerunJob = try await kernel.rerun(artifactID: original.id)
    #expect(rerunJob != firstJob)
    let job = try await runToDone(kernel, rerunJob)

    let rerunArtifact = try await resultArtifact(kernel, of: job)
    #expect(rerunArtifact.id != original.id)
    #expect(rerunArtifact.params == original.params)
    #expect(rerunArtifact.contentHash == original.contentHash)
    #expect(rerunArtifact.path == original.path)
    #expect(rerunArtifact.jobID == rerunJob)

    let blob = try Data(
        contentsOf: dir.appendingPathComponent("outputs").appendingPathComponent(original.path))
    #expect(blob == DeterministicImageAdapter.imageBytes(original.params))

    let reloaded = Kernel(directory: dir, adapters: [])
    let listed = try await reloaded.artifacts()
    #expect(listed.count == 2)
    #expect(Set(listed.map(\.id)) == [original.id, rerunArtifact.id])
    for artifact in listed {
        #expect(artifact.model == record.name)
        #expect(artifact.runtime == "fake:image")
        #expect(artifact.capability == .image)
        #expect(artifact.params == original.params)
        #expect(try seedValue(artifact.params) == 771_342)
    }
    #expect(Set(listed.map(\.jobID)) == [firstJob, rerunJob])
}

@Test func varyKeepsParamsButDrawsFreshSeed() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let firstJob = try await kernel.submit(record.id, .image, payload: imagePayload())
    _ = try await runToDone(kernel, firstJob)
    let original = try #require(try await kernel.artifacts().first)

    let varyJob = try await kernel.vary(artifactID: original.id)
    let job = try await runToDone(kernel, varyJob)
    let sibling = try await resultArtifact(kernel, of: job)

    #expect(try seedValue(sibling.params) != seedValue(original.params))
    guard case .object(let originalFields) = original.params,
        case .object(let siblingFields) = sibling.params
    else {
        Issue.record("expected object params")
        return
    }
    for key in ["prompt", "steps", "guidance", "size"] {
        #expect(siblingFields[key] == originalFields[key])
    }
    #expect(sibling.contentHash != original.contentHash)
    #expect(sibling.path != original.path)
}

@Test func submitWithoutSeedInjectsOneForProvenance() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(record.id, .image, payload: imagePayload(seed: nil))
    let job = try await runToDone(kernel, jobID)
    let artifact = try await resultArtifact(kernel, of: job)

    let seed = try seedValue(artifact.params)
    #expect(seed >= 0)
    #expect(try seedValue(job.payload) == seed)

    let rerunJob = try await kernel.rerun(artifactID: artifact.id)
    let rerun = try await runToDone(kernel, rerunJob)
    let reproduced = try await resultArtifact(kernel, of: rerun)
    #expect(reproduced.contentHash == artifact.contentHash)
}

@Test func deleteTrashesSidecarAndBlobOnlyWhenUnshared() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let firstJob = try await kernel.submit(record.id, .image, payload: imagePayload())
    _ = try await runToDone(kernel, firstJob)
    let original = try #require(try await kernel.artifacts().first)
    let rerunJob = try await kernel.rerun(artifactID: original.id)
    let rerun = try await runToDone(kernel, rerunJob)
    let duplicate = try await resultArtifact(kernel, of: rerun)

    let outputs = dir.appendingPathComponent("outputs")
    let blobURL = outputs.appendingPathComponent(original.path)
    let year = String(Calendar(identifier: .gregorian).component(.year, from: original.createdAt))

    try await kernel.deleteArtifact(id: duplicate.id)
    #expect(try await kernel.artifacts().map(\.id) == [original.id])
    #expect(
        !FileManager.default.fileExists(
            atPath: outputs.appendingPathComponent("\(year)/\(duplicate.id).json").path))
    #expect(FileManager.default.fileExists(atPath: blobURL.path))

    try await kernel.deleteArtifact(id: original.id)
    #expect(try await kernel.artifacts().isEmpty)
    #expect(!FileManager.default.fileExists(atPath: blobURL.path))
    #expect(
        !FileManager.default.fileExists(
            atPath: outputs.appendingPathComponent("\(year)/\(original.id).json").path))

    await #expect(throws: ArtifactStoreError.notFound(original.id)) {
        try await kernel.deleteArtifact(id: original.id)
    }
}

@Test func previewSpillsToContentAddressedBlobNotSidecar() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(record.id, .image, payload: imagePayload())
    let job = try await runToDone(kernel, jobID)
    let artifact = try await resultArtifact(kernel, of: job)

    let previewPath = try #require(artifact.previewPath)
    #expect(previewPath.hasPrefix("blobs/"))
    let outputs = dir.appendingPathComponent("outputs")
    let expected = DeterministicImageAdapter.previewBytes(artifact.params)
    #expect(try Data(contentsOf: outputs.appendingPathComponent(previewPath)) == expected)
    #expect(try await kernel.artifactStore.previewData(id: artifact.id) == expected)

    let year = String(Calendar(identifier: .gregorian).component(.year, from: artifact.createdAt))
    let sidecarData = try Data(
        contentsOf: outputs.appendingPathComponent("\(year)/\(artifact.id).json"))
    let sidecar = try JSONSerialization.jsonObject(with: sidecarData) as! [String: Any]
    #expect(sidecar["preview"] as? String == previewPath)
    #expect(sidecarData.count < 2048)
}

@Test func jobWithoutPreviewStoresArtifactWithoutPreviewRef() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir, emitPreview: false)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(record.id, .image, payload: imagePayload())
    let job = try await runToDone(kernel, jobID)
    let artifact = try await resultArtifact(kernel, of: job)
    #expect(artifact.previewPath == nil)
    #expect(try await kernel.artifactStore.previewData(id: artifact.id) == nil)
}

@Test func artifactURLPointsAtTheStoredBlob() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)
    let record = fakeImageRecord()
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(record.id, .image, payload: imagePayload())
    let job = try await runToDone(kernel, jobID)
    let artifact = try await resultArtifact(kernel, of: job)

    let url = try #require(try await kernel.artifactURL(id: artifact.id))
    #expect(url == dir.appendingPathComponent("outputs").appendingPathComponent(artifact.path))
    #expect(try Data(contentsOf: url) == DeterministicImageAdapter.imageBytes(artifact.params))
    #expect(try await kernel.artifactURL(id: "missing") == nil)
}

@Test func rerunAndVaryOnUnknownArtifactThrowArtifactNotFound() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = makeKernel(dir)

    await #expect(throws: KernelError.self) {
        _ = try await kernel.rerun(artifactID: "missing")
    }
    await #expect(throws: KernelError.self) {
        _ = try await kernel.vary(artifactID: "missing")
    }
}

@Test func listingIsSortedNewestFirstAcrossReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ArtifactStore(root: dir.appendingPathComponent("outputs"))

    for index in 0..<3 {
        _ = try await store.store(
            ArtifactDraft(
                data: Data("frame-\(index)".utf8),
                fileExtension: "png",
                model: "FLUX.1-schnell",
                modelID: "model",
                runtime: "fake:image",
                capability: .image,
                params: .object(["seed": .int(index)]),
                jobID: UUID().uuidString,
                durationMs: index))
        try await Task.sleep(for: .milliseconds(10))
    }

    let listed = try await store.list()
    #expect(listed.count == 3)
    #expect(listed.map(\.createdAt) == listed.map(\.createdAt).sorted(by: >))

    let reloaded = ArtifactStore(root: dir.appendingPathComponent("outputs"))
    #expect(try await reloaded.list().map(\.id) == listed.map(\.id))
}

@Test func identicalFramesFromOneJobGetDistinctArtifactIDs() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = ArtifactStore(root: dir.appendingPathComponent("outputs"))
    let jobID = UUID().uuidString
    let draft = ArtifactDraft(
        data: Data("frame".utf8),
        fileExtension: "png",
        model: "FLUX.1-schnell",
        modelID: "model",
        runtime: "fake:image",
        capability: .image,
        params: .object(["seed": .int(7)]),
        jobID: jobID,
        durationMs: 5)

    let first = try await store.store(draft)
    let second = try await store.store(draft)

    #expect(first.id != second.id)
    #expect(first.path == second.path)
    #expect(first.contentHash == second.contentHash)
    #expect(try await store.list().count == 2)

    let reloaded = ArtifactStore(root: dir.appendingPathComponent("outputs"))
    #expect(try await reloaded.list().count == 2)
}
