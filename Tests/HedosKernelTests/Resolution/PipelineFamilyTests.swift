import Foundation
import Testing

@testable import HedosKernel

private func hfDiffusersRecord(
    hub: URL, repo: String, modelIndex: String, scheduler: String?
) throws -> ModelRecord {
    try DiscoveryFixtures.makeHFRepo(
        at: hub,
        DiscoveryFixtures.HFRepo(
            org: "acme", repo: repo,
            files: [("weights.safetensors", 64)],
            modelIndexJSON: modelIndex,
            schedulerConfigJSON: scheduler))
    return ModelRecord(
        name: repo, modality: .unknown, capabilities: [],
        source: ModelSource(
            kind: .huggingfaceCache,
            path: hub.appendingPathComponent("models--acme--\(repo)").path,
            repo: "acme/\(repo)",
            ref: "abc123def456"))
}

private func folderDiffusersRecord(dir: URL, name: String, modelIndex: String) throws -> ModelRecord {
    let bundle = dir.appendingPathComponent(name)
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(modelIndex.utf8).write(to: bundle.appendingPathComponent("model_index.json"))
    return ModelRecord(
        name: name, modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
}

private func param(_ params: [ParamSpec], _ key: String) -> ParamSpec? {
    params.first { $0.key == key }
}

@Test func sdxlTurboSchedulerRefinesToTurboSchema() throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "sdxl-turbo",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.turboSchedulerConfig)

    let identified = Identification.identify(record)
    #expect(identified.format == .diffusers)
    #expect(identified.modality == .image)
    #expect(identified.capabilities == [.image])
    #expect(identified.pipelineClass == "StableDiffusionXLPipeline")
    let steps = try #require(param(identified.params, "steps"))
    #expect(steps.defaultValue == .int(2))
    #expect(steps.range == [.int(1), .int(8)])
    let guidance = try #require(param(identified.params, "guidance"))
    #expect(guidance.defaultValue == .double(0.0))
    #expect(guidance.range == [.double(0), .double(2)])
}

@Test func nonTurboSDXLWithTrailingEulerAncestralDoesNotGetTurboClamped() throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "realistic-vision-xl",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.turboSchedulerConfig)

    let identified = Identification.identify(record)
    #expect(identified.modality == .image)
    let steps = try #require(param(identified.params, "steps"))
    #expect(steps.defaultValue == .int(30))
    #expect(steps.range == [.int(1), .int(75)])
    let guidance = try #require(param(identified.params, "guidance"))
    #expect(guidance.defaultValue == .double(7.0))
}

@Test func substringNameSignalDoesNotClampUnrelatedRepo() throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "lcmix-photon-xl",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.turboSchedulerConfig)

    let steps = try #require(param(Identification.identify(record).params, "steps"))
    #expect(steps.defaultValue == .int(30))
    #expect(steps.range == [.int(1), .int(75)])

    let turbojet = try hfDiffusersRecord(
        hub: hub, repo: "turbojet-vision-xl",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.turboSchedulerConfig)
    let turbojetSteps = try #require(param(Identification.identify(turbojet).params, "steps"))
    #expect(turbojetSteps.range == [.int(1), .int(75)])

    let stillTurbo = try hfDiffusersRecord(
        hub: hub, repo: "SDXL-Lightning",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.turboSchedulerConfig)
    let lightningSteps = try #require(param(Identification.identify(stillTurbo).params, "steps"))
    #expect(lightningSteps.range == [.int(1), .int(8)])
}

@Test func baseSDXLLeadingSchedulerKeepsDefaultSchema() throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "sdxl-base",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: DiscoveryFixtures.sdxlBaseSchedulerConfig)

    let identified = Identification.identify(record)
    #expect(identified.modality == .image)
    let steps = try #require(param(identified.params, "steps"))
    #expect(steps.defaultValue == .int(30))
    #expect(steps.range == [.int(1), .int(75)])
    let guidance = try #require(param(identified.params, "guidance"))
    #expect(guidance.defaultValue == .double(7.0))
}

@Test func sdxlWithoutSchedulerConfigKeepsDefaultSchema() throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "sdxl-bare",
        modelIndex: DiscoveryFixtures.sdxlModelIndex,
        scheduler: nil)

    let identified = Identification.identify(record)
    #expect(identified.modality == .image)
    let steps = try #require(param(identified.params, "steps"))
    #expect(steps.defaultValue == .int(30))
    let guidance = try #require(param(identified.params, "guidance"))
    #expect(guidance.defaultValue == .double(7.0))
}

@Test func unknownPipelineClassCarriesClassAsDiagnostic() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try folderDiffusersRecord(
        dir: dir, name: "mystery", modelIndex: #"{"_class_name": "SomePipeline"}"#)

    let identified = Identification.identify(record)
    #expect(identified.format == .diffusers)
    #expect(identified.modality == nil)
    #expect(identified.capabilities.isEmpty)
    #expect(identified.params.isEmpty)
    #expect(identified.pipelineClass == "SomePipeline")
}

@Test func identifyOnlyVideoPipelineNamesModalityWithoutCapabilities() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let record = try folderDiffusersRecord(
        dir: dir, name: "cogvideo", modelIndex: DiscoveryFixtures.cogVideoModelIndex)

    let identified = Identification.identify(record)
    #expect(identified.modality == .video)
    #expect(identified.capabilities.isEmpty)

    let registry = Registry(directory: dir.appendingPathComponent("store"))
    try await registry.register(record)
    try await ResolutionEngine(adapters: [MfluxAdapter()]).resolveAll(in: registry)
    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.tier == .recipeNeeded)
    #expect(resolved.state == .unresolved)
    #expect(resolved.modality == .video)
}

@Test func fluxProfileIsUnchangedAndResolvesToMflux() async throws {
    let hub = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: hub) }
    let record = try hfDiffusersRecord(
        hub: hub, repo: "flux-schnell",
        modelIndex: DiscoveryFixtures.fluxModelIndex,
        scheduler: nil)

    let identified = Identification.identify(record)
    #expect(identified.params == PipelineFamilyRegistry.fluxParams)

    let registry = Registry(directory: hub.appendingPathComponent("store"))
    try await registry.register(record)
    try await ResolutionEngine(adapters: [MfluxAdapter()]).resolveAll(in: registry)
    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "python:mflux")
    #expect(resolved.runtime.tier == .managed)
    #expect(resolved.runtime.alternatives == ["python:diffusers"])
}

@Test func seededFamilyTableIsInternallyConsistent() {
    var seen: Set<String> = []
    for family in PipelineFamilyRegistry.builtin.families {
        for className in family.classNames {
            #expect(!seen.contains(className), "\(className) appears in two families")
            seen.insert(className)
        }
        if family.capabilities.isEmpty {
            #expect(family.params.isEmpty, "\(family.id) is identify-only but carries params")
        } else {
            #expect(!family.params.isEmpty, "\(family.id) is runnable but has no params")
            #expect(
                family.params.contains { $0.key == "seed" },
                "\(family.id) params lack a seed spec")
        }
        let keys = Set(family.params.map(\.key))
        for refinement in family.refinements {
            for override in refinement.paramOverrides {
                #expect(
                    keys.contains(override.key),
                    "\(family.id) refinement overrides unknown key \(override.key)")
            }
        }
    }
}
