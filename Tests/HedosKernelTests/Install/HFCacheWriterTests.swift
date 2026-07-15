import Foundation
import Testing

@testable import HedosKernel

private func tempRoot() throws -> URL {
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("hedos-hf-writer-\(UUID().uuidString)")
        .appendingPathComponent("hub")
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

private func install(
    _ provider: HuggingFaceInstallProvider, reference: String
) async throws -> [InstallStreamEvent] {
    let plan = try await provider.plan(reference: reference)
    var events: [InstallStreamEvent] = []
    for try await event in provider.install(plan) {
        events.append(event)
    }
    return events
}

private func scan(_ root: URL) async -> ScanResult {
    await HFCacheScanner(root: root).scan()
}

struct HFCacheWriterTests {
    @Test func fullInstallProducesScannerReadableLayout() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles()
        let (provider, _) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)
        let events = try await install(provider, reference: "org/tiny")
        #expect(events.contains { if case .progress = $0 { true } else { false } })

        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        let refs = try String(
            contentsOf: layout.refsDirectory.appendingPathComponent("main"), encoding: .utf8)
        #expect(refs == "rev0123abc")
        for (path, content) in files {
            let snapshot = layout.snapshotFile(revision: "rev0123abc", path: path)
            let restored = try Data(contentsOf: snapshot)
            #expect(restored == content, "\(path) should round-trip through the blob store")
            let blob = layout.blobURL(named: sha256Hex(content))
            #expect(FileManager.default.fileExists(atPath: blob.path))
        }
        let result = await scan(root)
        let model = result.discovered.first { $0.source.repo == "org/tiny" }
        #expect(model != nil)
        #expect(model?.downloading == false)
        #expect(model?.contextLengthHint == 2048)
    }

    @Test func skeletonAloneReadsAsDownloading() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        let writer = HFCacheWriter(
            layout: layout, transport: FakeHubTransport(repos: [:]))
        let weight = HFSibling(rfilename: "model.safetensors", size: 64)
        try writer.prepareSkeleton(
            revision: "rev0123abc",
            firstWeightPendingName: HFCacheWriter.pendingBlobName(for: weight))
        let result = await scan(root)
        let model = result.discovered.first { $0.source.repo == "org/tiny" }
        #expect(model != nil)
        #expect(model?.downloading == true)
    }

    @Test func preseededIncompleteResumesWithRangeRequest() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 4096)
        let weightContent = files["model.safetensors"]!
        let (provider, transport) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)

        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        try FileManager.default.createDirectory(
            at: layout.blobsDirectory, withIntermediateDirectories: true)
        let pending = layout.incompleteURL(named: sha256Hex(weightContent))
        try weightContent.prefix(1000).write(to: pending)

        let plan = try await provider.plan(reference: "org/tiny")
        #expect(plan.remainingBytes == (plan.totalBytes ?? 0) - 1000)
        var lastProgress: InstallProgress?
        for try await event in provider.install(plan) {
            if case .progress(let progress) = event { lastProgress = progress }
        }
        let ranged = transport.recordedRequests.compactMap {
            $0.value(forHTTPHeaderField: "Range")
        }
        #expect(ranged == ["bytes=1000-"])
        #expect(lastProgress?.bytesDownloaded == lastProgress?.totalBytes)
        let restored = try Data(
            contentsOf: layout.snapshotFile(revision: "rev0123abc", path: "model.safetensors"))
        #expect(restored == weightContent)
    }

    @Test func interruptionNeverRemovesAPreexistingRepo() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 512)
        let (goodProvider, _) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)
        _ = try await install(goodProvider, reference: "org/tiny")

        var updated = files
        updated["model.safetensors"] = Data(repeating: 0xEE, count: 512)
        let (failingProvider, transport) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: updated, revision: "rev0456def")],
            root: root)
        transport.failPath("model.safetensors")
        do {
            _ = try await install(failingProvider, reference: "org/tiny")
            Issue.record("install should have thrown")
        } catch {}
        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        #expect(FileManager.default.fileExists(atPath: layout.repoDirectory.path))
        let restored = try Data(
            contentsOf: layout.snapshotFile(revision: "rev0123abc", path: "model.safetensors"))
        #expect(restored == files["model.safetensors"])
    }

    @Test func serverIgnoringRangeRestartsCleanly() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 4096)
        let weightContent = files["model.safetensors"]!
        let (provider, _) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root,
            honorRange: false)

        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        try FileManager.default.createDirectory(
            at: layout.blobsDirectory, withIntermediateDirectories: true)
        let pending = layout.incompleteURL(named: sha256Hex(weightContent))
        try weightContent.prefix(1000).write(to: pending)

        _ = try await install(provider, reference: "org/tiny")
        let restored = try Data(
            contentsOf: layout.snapshotFile(revision: "rev0123abc", path: "model.safetensors"))
        #expect(restored == weightContent)
    }

    @Test func corruptedTransferThrowsChecksumMismatchAndCleansBlob() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 512)
        let (provider, transport) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)
        transport.corruptPath("model.safetensors")
        do {
            _ = try await install(provider, reference: "org/tiny")
            Issue.record("install should have thrown")
        } catch let error as InstallError {
            #expect(error == .checksumMismatch(file: "model.safetensors"))
        }
        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        #expect(!FileManager.default.fileExists(atPath: layout.repoDirectory.path))
    }

    @Test func failureBeforeAnyWeightRemovesRepo() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 512)
        let (provider, transport) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)
        transport.failPath("model.safetensors")
        do {
            _ = try await install(provider, reference: "org/tiny")
            Issue.record("install should have thrown")
        } catch {}
        let layout = HFCacheLayout(root: root, repo: "org/tiny")
        #expect(!FileManager.default.fileExists(atPath: layout.repoDirectory.path))
        let result = await scan(root)
        #expect(result.discovered.isEmpty)
    }

    @Test func failureAfterAWeightKeepsPartialRepoAndResumeSkipsIt() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let bigWeight = Data(repeating: 0xCD, count: Int(11 << 20))
        var files = HFInstallFixtures.tinyRepoFiles(weightBytes: 512)
        files["model-00001-of-00002.safetensors"] = bigWeight
        files["model-00002-of-00002.safetensors"] = Data(repeating: 0xEF, count: Int(12 << 20))
        files.removeValue(forKey: "model.safetensors")
        let (provider, transport) = HFInstallFixtures.provider(
            repos: ["org/big": FakeHubTransport.Repo(files: files)], root: root)
        transport.failPath("model-00002-of-00002.safetensors")
        do {
            _ = try await install(provider, reference: "org/big")
            Issue.record("install should have thrown")
        } catch {}
        let layout = HFCacheLayout(root: root, repo: "org/big")
        #expect(FileManager.default.fileExists(atPath: layout.repoDirectory.path))
        #expect(
            FileManager.default.fileExists(
                atPath: layout.blobURL(named: sha256Hex(bigWeight)).path))

        transport.healPath("model-00002-of-00002.safetensors")
        _ = try await install(provider, reference: "org/big")
        let firstWeightFetches = transport.recordedRequests.filter {
            $0.url?.path.hasSuffix("model-00001-of-00002.safetensors") == true
        }
        #expect(firstWeightFetches.count == 1)
        let result = await scan(root)
        let model = result.discovered.first { $0.source.repo == "org/big" }
        #expect(model?.downloading == false)
    }

    @Test func gatedRepoWithoutTokenThrowsAuthRequired() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let (provider, _) = HFInstallFixtures.provider(
            repos: [
                "org/gated": FakeHubTransport.Repo(
                    files: HFInstallFixtures.tinyRepoFiles(), gated: true)
            ], root: root)
        await #expect(throws: InstallError.authRequired("org/gated")) {
            _ = try await provider.plan(reference: "org/gated")
        }
    }

    @Test func gatedRepoWithTokenPlansWithAuthSatisfied() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let (provider, transport) = HFInstallFixtures.provider(
            repos: [
                "org/gated": FakeHubTransport.Repo(
                    files: HFInstallFixtures.tinyRepoFiles(), gated: true)
            ], root: root, token: "hf_test")
        let plan = try await provider.plan(reference: "org/gated")
        #expect(!plan.requiresAuth)
        #expect(
            transport.recordedRequests.allSatisfy {
                $0.value(forHTTPHeaderField: "Authorization") == "Bearer hf_test"
            })
    }

    @Test func planCarriesFilesSizesAndRevision() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let files = HFInstallFixtures.tinyRepoFiles(weightBytes: 4096)
        let (provider, _) = HFInstallFixtures.provider(
            repos: ["org/tiny": FakeHubTransport.Repo(files: files)], root: root)
        let plan = try await provider.plan(reference: "org/tiny")
        #expect(plan.revision == "rev0123abc")
        #expect(plan.files.count == 3)
        #expect(plan.totalBytes == files.values.map { Int64($0.count) }.reduce(0, +))
        #expect(plan.destination.hasSuffix("hub"))
        #expect(!plan.requiresAuth)
    }

    @Test func unknownRepoThrowsReferenceNotFound() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let (provider, _) = HFInstallFixtures.provider(repos: [:], root: root)
        await #expect(throws: InstallError.referenceNotFound("org/absent")) {
            _ = try await provider.plan(reference: "org/absent")
        }
        await #expect(throws: InstallError.referenceInvalid("not-a-repo")) {
            _ = try await provider.plan(reference: "not-a-repo")
        }
    }

    @Test func searchParsesHits() async throws {
        let root = try tempRoot()
        defer { try? FileManager.default.removeItem(at: root.deletingLastPathComponent()) }
        let (provider, transport) = HFInstallFixtures.provider(repos: [:], root: root)
        transport.setSearchHits([
            [
                "id": "org/model-a", "downloads": 12345, "likes": 67,
                "lastModified": "2026-01-02T03:04:05.000Z",
            ],
            ["id": "org/model-b"],
        ])
        let hits = try await provider.search(matching: "model", limit: 10)
        #expect(hits.count == 2)
        #expect(hits[0].reference == "org/model-a")
        #expect(hits[0].name == "model-a")
        #expect(hits[0].downloads == 12345)
        #expect(hits[0].updatedAt != nil)
        #expect(hits[1].likes == nil)
    }
}
