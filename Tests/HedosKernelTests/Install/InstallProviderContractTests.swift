import Foundation
import Synchronization
import Testing

@testable import HedosKernel

struct InstallStreamContractTests {
    @Test func bodyErrorsPassThroughTheMapper() async {
        let stream = InstallStream.make { error in
            (error as? URLError).map { _ in InstallError.transferFailed("mapped") }
        } run: { _, _ in
            throw URLError(.timedOut)
        }
        do {
            for try await _ in stream {}
            Issue.record("stream should have thrown")
        } catch let error as InstallError {
            #expect(error == .transferFailed("mapped"))
        } catch {
            Issue.record("unexpected error \(error)")
        }
    }

    @Test func unmappedErrorsSurfaceUnchanged() async {
        let stream = InstallStream.make { _ in nil } run: { _, _ in
            throw InstallError.referenceInvalid("x")
        }
        do {
            for try await _ in stream {}
            Issue.record("stream should have thrown")
        } catch let error as InstallError {
            #expect(error == .referenceInvalid("x"))
        } catch {
            Issue.record("unexpected error \(error)")
        }
    }

    @Test func bodyCancellationRunsCleanupAndFinishesClean() async throws {
        let cleaned = Mutex(0)
        let stream = InstallStream.make { _ in nil } run: { continuation, interruption in
            interruption.register { cleaned.withLock { $0 += 1 } }
            continuation.yield(.status("working"))
            throw CancellationError()
        }
        var events: [InstallStreamEvent] = []
        for try await event in stream {
            events.append(event)
        }
        #expect(events == [.status("working")])
        #expect(cleaned.withLock { $0 } == 1)
    }

    @Test func consumerCancellationRunsCleanup() async throws {
        let cleaned = Mutex(0)
        let started = Mutex(false)
        let stream = InstallStream.make { _ in nil } run: { continuation, interruption in
            interruption.register { cleaned.withLock { $0 += 1 } }
            continuation.yield(.status("working"))
            started.withLock { $0 = true }
            try await Task.sleep(for: .seconds(300))
        }
        let consumer = Task {
            do {
                for try await _ in stream {}
            } catch {}
        }
        for _ in 0..<2000 {
            if started.withLock({ $0 }) { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(started.withLock { $0 })
        consumer.cancel()
        await consumer.value
        for _ in 0..<2000 {
            if cleaned.withLock({ $0 }) >= 1 { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        #expect(cleaned.withLock { $0 } >= 1)
    }

    @Test func successRunsNoCleanup() async throws {
        let cleaned = Mutex(0)
        let stream = InstallStream.make { _ in nil } run: { continuation, interruption in
            interruption.register { cleaned.withLock { $0 += 1 } }
            continuation.yield(.progress(InstallProgress(bytesDownloaded: 5)))
        }
        var events: [InstallStreamEvent] = []
        for try await event in stream {
            events.append(event)
        }
        #expect(events.count == 1)
        #expect(cleaned.withLock { $0 } == 0)
    }
}

struct InstallProviderContractTests {
    static func providers() -> [any InstallProvider] {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("hedos-contract-\(UUID().uuidString)")
        let transport = FakeHubTransport(repos: [
            "org/repo": FakeHubTransport.Repo(files: HFInstallFixtures.tinyRepoFiles())
        ])
        return [
            OllamaInstallProvider(),
            HuggingFaceInstallProvider(root: root, transport: transport),
        ]
    }

    @Test func identityIsStableAndNamed() {
        for provider in Self.providers() {
            #expect(!provider.id.rawValue.isEmpty)
            #expect(!provider.displayName.isEmpty)
        }
    }

    @Test func sourceKindMatchesAScannedHabitat() {
        let scanned: Set<SourceKind> = [
            .ollama, .huggingfaceCache, .lmStudio, .file, .folder,
        ]
        for provider in Self.providers() {
            #expect(scanned.contains(provider.sourceKind))
        }
    }

    @Test func malformedReferenceThrowsInstallError() async {
        for provider in Self.providers() {
            do {
                _ = try await provider.plan(reference: "  ")
                Issue.record("\(provider.id) accepted a blank reference")
            } catch is InstallError {
            } catch {
                Issue.record("\(provider.id) threw \(error) instead of InstallError")
            }
        }
    }

    @Test func searchlessProvidersThrowInsteadOfReturningEmpty() async {
        for provider in Self.providers() where !provider.supportsSearch {
            do {
                _ = try await provider.search(matching: "gemma", limit: 5)
                Issue.record("\(provider.id) returned from search despite supportsSearch == false")
            } catch is InstallError {
            } catch {
                Issue.record("\(provider.id) threw \(error) instead of InstallError")
            }
        }
    }

    @Test func plansCarryProviderIdentityAndDestination() async throws {
        let references: [InstallProviderID: String] = [
            .ollama: "gemma3:4b",
            .huggingface: "org/repo",
        ]
        for provider in Self.providers() {
            guard let reference = references[provider.id] else {
                Issue.record("no contract reference for \(provider.id)")
                continue
            }
            let plan = try await provider.plan(reference: reference)
            #expect(plan.provider == provider.id)
            #expect(!plan.destination.isEmpty)
            #expect(!plan.displayName.isEmpty)
        }
    }
}
