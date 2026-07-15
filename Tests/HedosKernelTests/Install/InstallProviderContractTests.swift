import Foundation
import Testing

@testable import HedosKernel

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
