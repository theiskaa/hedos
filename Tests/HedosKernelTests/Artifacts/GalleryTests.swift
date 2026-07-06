import Foundation
import Testing

@testable import HedosKernel

private func stubArtifact(
    id: String,
    model: String = "FLUX.1-schnell",
    modelID: String = "flux",
    createdAt: Date,
    params: JSONValue = .object(["prompt": .string("a lighthouse at dusk")])
) -> Artifact {
    Artifact(
        id: id,
        path: "2026/\(id).png",
        contentHash: id,
        model: model,
        modelID: modelID,
        runtime: "fake:image",
        capability: .image,
        params: params,
        createdAt: createdAt,
        durationMs: 100,
        jobID: UUID().uuidString)
}

private func fluxPayload(prompt: String, seed: Int = 771_342) -> JSONValue {
    .object([
        "prompt": .string(prompt),
        "steps": .int(4),
        "guidance": .double(0.0),
        "size": .string("1024x1024"),
        "seed": .int(seed),
    ])
}

@Test func galleryModelsAreDistinctAndOrderedByRecency() {
    let base = Date(timeIntervalSince1970: 1_750_000_000)
    let artifacts = [
        stubArtifact(id: "a", modelID: "flux", createdAt: base),
        stubArtifact(
            id: "b", model: "sdxl-turbo", modelID: "sdxl", createdAt: base.addingTimeInterval(20)),
        stubArtifact(id: "c", modelID: "flux", createdAt: base.addingTimeInterval(40)),
    ]

    let models = Gallery.models(in: artifacts)
    #expect(models == [
        GalleryModel(id: "flux", name: "FLUX.1-schnell"),
        GalleryModel(id: "sdxl", name: "sdxl-turbo"),
    ])
    #expect(Gallery.models(in: []).isEmpty)
}

@Test func galleryArrangeFiltersByModel() {
    let base = Date(timeIntervalSince1970: 1_750_000_000)
    let artifacts = [
        stubArtifact(id: "a", modelID: "flux", createdAt: base),
        stubArtifact(
            id: "b", model: "sdxl-turbo", modelID: "sdxl", createdAt: base.addingTimeInterval(20)),
        stubArtifact(id: "c", modelID: "flux", createdAt: base.addingTimeInterval(40)),
    ]

    #expect(Gallery.arrange(artifacts, modelID: "flux").map(\.id) == ["c", "a"])
    #expect(Gallery.arrange(artifacts, modelID: "sdxl").map(\.id) == ["b"])
    #expect(Gallery.arrange(artifacts, modelID: "missing").isEmpty)
    #expect(Gallery.arrange(artifacts, modelID: nil).count == 3)
}

@Test func galleryArrangeSortsByDateBothWaysWithStableTieBreak() {
    let base = Date(timeIntervalSince1970: 1_750_000_000)
    let artifacts = [
        stubArtifact(id: "old", createdAt: base),
        stubArtifact(id: "tie-b", createdAt: base.addingTimeInterval(20)),
        stubArtifact(id: "tie-a", createdAt: base.addingTimeInterval(20)),
        stubArtifact(id: "new", createdAt: base.addingTimeInterval(40)),
    ]

    let newest = Gallery.arrange(artifacts, sort: .newestFirst)
    #expect(newest.map(\.id) == ["new", "tie-b", "tie-a", "old"])

    let oldest = Gallery.arrange(artifacts, sort: .oldestFirst)
    #expect(oldest.map(\.id) == ["old", "tie-a", "tie-b", "new"])
}

@Test func artifactParamsRoundTripThroughTheCanvasForm() {
    let record = Fixtures.flux()
    let payload = fluxPayload(prompt: "a lighthouse at dusk")
    let artifact = stubArtifact(
        id: "a", createdAt: Date(timeIntervalSince1970: 1_750_000_000), params: payload)

    var form = ParamForm(schema: record.params)
    form.load(artifact.params)
    let prompt = Provenance.prompt(of: artifact.params) ?? ""

    #expect(prompt == "a lighthouse at dusk")
    #expect(form.payload(prompt: prompt) == payload)
    #expect(form.int("steps") == 4)
    #expect(form.double("guidance") == 0.0)
    #expect(form.string("size") == "1024x1024")
    #expect(form.int("seed") == 771_342)
}

@Test func kernelExposesPreviewDataForGalleryThumbnails() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [DeterministicImageAdapter()])
    var record = Fixtures.flux()
    record.runtime = RuntimeRef(id: "fake:image", resolved: .auto, tier: .managed)
    try await kernel.registry.register(record)

    let jobID = try await kernel.submit(
        record.id, .image, payload: fluxPayload(prompt: "a lighthouse at dusk"))
    for await _ in await kernel.jobEvents(id: jobID) {}
    let job = try #require(try await kernel.job(id: jobID))
    let artifactID = try #require(job.result.first)
    let artifact = try #require(try await kernel.artifact(id: artifactID))

    let preview = try #require(try await kernel.artifactPreview(id: artifactID))
    #expect(preview == DeterministicImageAdapter.previewBytes(artifact.params))
    #expect(try await kernel.artifactPreview(id: "missing") == nil)
}

@Test func galleryGateRunsAcrossPromptsThenDeletesAndReruns() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [DeterministicImageAdapter()])
    var record = Fixtures.flux()
    record.runtime = RuntimeRef(id: "fake:image", resolved: .auto, tier: .managed)
    try await kernel.registry.register(record)

    let prompts = ["a lighthouse at dusk", "a fox in the snow", "a lighthouse at dusk"]
    for (index, prompt) in prompts.enumerated() {
        let jobID = try await kernel.submit(
            record.id, .image, payload: fluxPayload(prompt: prompt, seed: index))
        for await _ in await kernel.jobEvents(id: jobID) {}
    }

    let arranged = Gallery.arrange(try await kernel.artifacts())
    #expect(arranged.count == 3)
    #expect(
        Set(arranged.compactMap { Provenance.prompt(of: $0.params) })
            == ["a lighthouse at dusk", "a fox in the snow"])
    for artifact in arranged {
        #expect(artifact.model == record.name)
        #expect(artifact.modelID == record.id)
        #expect(artifact.runtime == "fake:image")
        #expect(artifact.capability == .image)
    }
    #expect(Gallery.models(in: arranged) == [GalleryModel(id: record.id, name: record.name)])

    let doomed = try #require(arranged.last)
    try await kernel.deleteArtifact(id: doomed.id)
    let remaining = Gallery.arrange(try await kernel.artifacts())
    #expect(remaining.count == 2)
    #expect(!remaining.map(\.id).contains(doomed.id))

    let kept = try #require(remaining.first)
    let rerunJob = try await kernel.rerun(artifactID: kept.id)
    for await _ in await kernel.jobEvents(id: rerunJob) {}
    let job = try #require(try await kernel.job(id: rerunJob))
    let reproducedID = try #require(job.result.first)
    let reproduced = try #require(try await kernel.artifact(id: reproducedID))
    #expect(reproduced.contentHash == kept.contentHash)
    #expect(reproduced.params == kept.params)
    #expect(Gallery.arrange(try await kernel.artifacts()).count == 3)
}
