import Foundation
import Synchronization
import Testing

@testable import HedosKernel

struct OllamaRemovalTests {
    private func makeOllamaHome() throws -> URL {
        let home = try RemovalFixtures.tempDirectory("ollama-home")
        try DiscoveryFixtures.makeOllamaStore(
            at: home.appendingPathComponent(".ollama/models"),
            tags: [.init(model: "gemma4", tag: "latest", modelBytes: 2048)])
        return home
    }

    @Test func ollamaDeleteSendsModelTagAndNeverTouchesDisk() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport)
        transport.onDelete {
            try? FileManager.default.removeItem(
                at: home.appendingPathComponent(".ollama/models/manifests"))
        }
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        let report = try await kernel.deleteModel(record.id)

        let deletes = transport.deleteRequests
        #expect(deletes.count == 1)
        #expect(deletes.first?.httpMethod == "DELETE")
        let body = try JSONSerialization.jsonObject(
            with: deletes.first?.httpBody ?? Data()) as? [String: String]
        #expect(body == ["model": record.source.repo ?? record.name])
        #expect(report.daemonDeleted)
        #expect(report.trashedPaths.isEmpty)
        #expect(trasher.trashed.isEmpty)
        #expect(
            FileManager.default.fileExists(
                atPath: home.appendingPathComponent(".ollama/models/blobs").path))
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func ollamaDelete404StillUnregisters() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.setDeleteResponse(status: 404, body: #"{"error":"model not found"}"#)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)
        try FileManager.default.removeItem(
            at: home.appendingPathComponent(".ollama/models/manifests"))

        let report = try await kernel.deleteModel(record.id)

        #expect(report.daemonDeleted)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func ollamaDeleteDaemonDownWithoutBinaryThrows() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.setReachable(false)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport, binaryPresent: false)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        do {
            _ = try await kernel.deleteModel(record.id)
            Issue.record("delete should have thrown")
        } catch let error as RemovalError {
            guard case .daemonUnavailable = error else {
                Issue.record("unexpected error \(error)")
                return
            }
        }
        #expect(try await kernel.registry.get(id: record.id) != nil)
        #expect(transport.deleteRequests.isEmpty)
    }

    @Test func ollamaTransportFailureMapsToRemovalError() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.failDelete(URLError(.networkConnectionLost))
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        do {
            _ = try await kernel.deleteModel(record.id)
            Issue.record("delete should have thrown")
        } catch let error as RemovalError {
            guard case .daemonDeleteFailed = error else {
                Issue.record("unexpected RemovalError \(error)")
                return
            }
        } catch {
            Issue.record("leaked non-RemovalError \(error)")
        }
        #expect(try await kernel.registry.get(id: record.id) != nil)
    }

    @Test func ollamaDeleteDaemonErrorSurfacesMessage() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.setDeleteResponse(status: 500, body: #"{"error":"boom"}"#)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        await #expect(throws: RemovalError.daemonDeleteFailed("ollama: boom")) {
            _ = try await kernel.deleteModel(record.id)
        }
        #expect(try await kernel.registry.get(id: record.id) != nil)
    }

    @Test func ollamaDeleteStartsDaemonWhenBinaryExists() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.setReachable(false)
        let started = Mutex(false)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport, binaryPresent: true,
            startDaemon: {
                started.withLock { $0 = true }
                transport.setReachable(true)
            })
        transport.onDelete {
            try? FileManager.default.removeItem(
                at: home.appendingPathComponent(".ollama/models/manifests"))
        }
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        _ = try await kernel.deleteModel(record.id)

        #expect(started.withLock { $0 })
        #expect(transport.deleteRequests.count == 1)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }

    @Test func deleteRefusesWhileAnInstallForTheSameModelIsInFlight() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let held = HeldInstallProvider(id: .ollama, sourceKind: .ollama)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, installProviders: [held])
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)

        let plan = InstallPlan(
            provider: .ollama, reference: "gemma4:latest", displayName: "gemma4:latest",
            destination: "~/.ollama/models")
        _ = try await kernel.installs.begin(plan)
        try await Task.sleep(for: .milliseconds(50))

        await #expect(throws: RemovalError.stillDownloading(name: record.displayName)) {
            _ = try await kernel.deleteModel(record.id)
        }
        #expect(try await kernel.registry.get(id: record.id) != nil)
        held.release()
    }

    @Test func missingOllamaRecordSkipsDaemon() async throws {
        let home = try makeOllamaHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let trasher = FakeTrasher(bin: home.appendingPathComponent("bin"))
        let transport = FakeOllamaTransport()
        transport.setReachable(false)
        let kernel = await RemovalFixtures.makeKernel(
            home: home, directory: home.appendingPathComponent("support"),
            trasher: trasher, transport: transport)
        _ = try await kernel.discover()
        let record = try await RemovalFixtures.onlyRecord(kernel, kind: .ollama)
        try FileManager.default.removeItem(
            at: home.appendingPathComponent(".ollama/models/manifests"))
        _ = try await kernel.registry.setStateIfPresent(id: record.id, to: .missing)

        let report = try await kernel.deleteModel(record.id)

        #expect(!report.daemonDeleted)
        #expect(transport.deleteRequests.isEmpty)
        #expect(try await kernel.registry.get(id: record.id) == nil)
    }
}
