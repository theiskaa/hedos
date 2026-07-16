import Foundation
import Synchronization
import Testing

@testable import HedosKernel

struct ModelRemoverTests {
    @Test func deleteHFRepoTrashesRepoDirectoryAndForgetsRecord() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let hub = home.appendingPathComponent("hub")
        try DiscoveryFixtures.makeHFRepo(
            at: hub,
            .init(
                org: "org", repo: "tiny",
                files: [(name: "model.safetensors", bytes: 4096)],
                configJSON: DiscoveryFixtures.causalLMConfig))
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeHFKernel(
            home: home, directory: home.appendingPathComponent("support"),
            hubRoot: hub, trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .huggingfaceCache)

        let report = try await kernel.deleteModel(record.id)

        #expect(RemovalFixtures.canon(report.trashedPaths) == RemovalFixtures.canon([record.source.path]))
        #expect(!FileManager.default.fileExists(atPath: record.source.path))
        #expect(try await kernel.registry.get(id: record.id) == nil)
        #expect(!report.daemonDeleted)
    }

    @Test func deleteLooseGGUFTrashesSingleFile() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "tiny.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        let report = try await kernel.deleteModel(record.id)

        #expect(
            RemovalFixtures.canon(report.trashedPaths)
                == RemovalFixtures.canon([models.appendingPathComponent("tiny.gguf").path]))
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteShardedRecordTrashesEveryShard() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        let shards = try DiscoveryFixtures.makeShardedGGUF(
            at: models, baseName: "big", parts: 3)
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        let report = try await kernel.deleteModel(record.id)

        #expect(
            RemovalFixtures.canon(report.trashedPaths)
                == RemovalFixtures.canon(shards.map(\.path)))
        #expect(report.trashedPaths.count == 3)
        for shard in shards {
            #expect(!FileManager.default.fileExists(atPath: shard.path))
        }
    }

    @Test func deleteShardedRecordWithMissingPartTrashesPresentShards() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        let shards = try DiscoveryFixtures.makeShardedGGUF(
            at: models, baseName: "big", parts: 3, presentParts: [1, 3])
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        let report = try await kernel.deleteModel(record.id)

        #expect(
            RemovalFixtures.canon(report.trashedPaths)
                == RemovalFixtures.canon(shards.map(\.path)))
        #expect(report.trashedPaths.count == 2)
    }

    @Test func deleteFolderBundleTrashesDirectory() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let bundle = home.appendingPathComponent("Models/bundle")
        try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
        try Data(DiscoveryFixtures.causalLMConfig.utf8).write(
            to: bundle.appendingPathComponent("config.json"))
        try DiscoveryFixtures.data(bytes: 4096).write(
            to: bundle.appendingPathComponent("model.safetensors"))
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .folder)

        let report = try await kernel.deleteModel(record.id)

        #expect(RemovalFixtures.canon(report.trashedPaths) == RemovalFixtures.canon([bundle.path]))
        #expect(!FileManager.default.fileExists(atPath: bundle.path))
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteMissingShardedRecordTrashesSurvivingShards() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        let shards = try DiscoveryFixtures.makeShardedGGUF(
            at: models, baseName: "big", parts: 3)
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)
        try FileManager.default.removeItem(at: shards[0])
        _ = try await kernel.registry.setStateIfPresent(id: record.id, to: .missing)

        let report = try await kernel.deleteModel(record.id)

        #expect(
            RemovalFixtures.canon(report.trashedPaths)
                == RemovalFixtures.canon([shards[1].path, shards[2].path]))
        #expect(report.freedBytesEstimate > 0)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteShardedRecordWithUppercaseExtensionTrashesEveryShard() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try FileManager.default.createDirectory(at: models, withIntermediateDirectories: true)
        var written: [URL] = []
        for part in 1...2 {
            let name = String(format: "Model-%05d-of-%05d.GGUF", part, 2)
            let url = models.appendingPathComponent(name)
            var builder = GGUFFixtureBuilder(keyValueCount: 1)
            builder.addString(key: "general.architecture", value: "llama")
            try builder.write(to: url, trailingBytes: 1024)
            written.append(url)
        }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let paths = ModelRemover.removablePaths(
            for: ModelRecord(
                name: "Model", modality: .text, capabilities: [.chat],
                source: ModelSource(kind: .file, path: written[0].path),
                state: .ready))
        #expect(
            RemovalFixtures.canon(paths.map(\.path))
                == RemovalFixtures.canon(written.map(\.path)))
    }

    @Test func deleteMissingRecordForgetsWithoutDiskWork() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "gone.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)
        try FileManager.default.removeItem(atPath: record.source.path)
        _ = try await kernel.registry.setStateIfPresent(id: record.id, to: .missing)

        let report = try await kernel.deleteModel(record.id)

        #expect(report.trashedPaths.isEmpty)
        #expect(trasher.trashed.isEmpty)
        #expect(report.freedBytesEstimate == 0)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteBusyModelThrowsAndKeepsEverything() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "busy.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)
        await kernel.governor.markLoaded(
            modelID: record.id, name: record.name, footprintMB: 1) {}
        await kernel.governor.beginGeneration(record.id)

        await #expect(throws: RemovalError.modelBusy(name: record.displayName)) {
            _ = try await kernel.deleteModel(record.id)
        }
        #expect(FileManager.default.fileExists(atPath: record.source.path))
        #expect(try await kernel.registry.get(id: record.id) != nil)

        await kernel.governor.endGeneration(record.id)
        _ = try await kernel.deleteModel(record.id)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteResidentIdleModelEvictsThenDeletes() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "warm.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)
        let unloaded = Mutex(false)
        await kernel.governor.markLoaded(
            modelID: record.id, name: record.name, footprintMB: 1
        ) {
            unloaded.withLock { $0 = true }
        }

        _ = try await kernel.deleteModel(record.id)

        #expect(unloaded.withLock { $0 })
        #expect(await !kernel.governor.isResident(record.id))
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func builtinAndEndpointAreRefused() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        let endpoint = try await kernel.registerEndpoint(
            baseURL: "127.0.0.1:9999/v1", model: "remote")

        await #expect(throws: RemovalError.notDeletable(kind: .endpoint)) {
            _ = try await kernel.deleteModel(endpoint.id)
        }

        let builtin = ModelRecord(
            name: "apple", modality: .text, capabilities: [.chat],
            source: ModelSource(
                kind: .builtin,
                path: "/System/Library/Frameworks/FoundationModels.framework"),
            state: .ready)
        try await kernel.registry.register(builtin)
        await #expect(throws: RemovalError.notDeletable(kind: .builtin)) {
            _ = try await kernel.deleteModel(builtin.id)
        }
    }

    @Test func trashFailureKeepsRecordRegistered() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        _ = try DiscoveryFixtures.makeShardedGGUF(at: models, baseName: "big", parts: 3)
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        trasher.failOnPathSuffix("big-00002-of-00003.gguf")
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        do {
            _ = try await kernel.deleteModel(record.id)
            Issue.record("delete should have thrown")
        } catch let error as RemovalError {
            guard case .trashFailed = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
        #expect(try await kernel.registry.get(id: record.id) != nil)
    }

    @Test func deletedModelDoesNotResurrectOnRescan() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "tiny.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        _ = try await kernel.deleteModel(record.id)
        await kernel.scopedRescan([.file])

        #expect(try await kernel.registry.get(id: record.id) == nil)
        #expect(try await kernel.shelf().filter { $0.source.kind == .file }.isEmpty)
    }

    @Test func deleteClearsDefaultChatModel() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        try DiscoveryFixtures.makeGGUF(architecture: "llama", at: models, name: "tiny.gguf")
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)
        try await kernel.settings.setDefaultChatModelID(record.id)

        _ = try await kernel.deleteModel(record.id)

        #expect(await kernel.settings.defaultChatModelID() == nil)
    }

    @Test func deletionPreviewListsShardPathsAndBytes() async throws {
        let home = try RemovalFixtures.tempDirectory("home")
        defer { try? FileManager.default.removeItem(at: home) }
        let models = home.appendingPathComponent("Models")
        let shards = try DiscoveryFixtures.makeShardedGGUF(
            at: models, baseName: "big", parts: 3)
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"), trasher: trasher)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .file)

        let preview = try await kernel.deletionPreview(record.id)

        #expect(
            RemovalFixtures.canon(preview.paths)
                == RemovalFixtures.canon(shards.map(\.path)))
        #expect(preview.bytesEstimate == Int64(record.footprintMB ?? 0) << 20)
        #expect(!preview.viaDaemon)
        #expect(!preview.missing)
    }
}
