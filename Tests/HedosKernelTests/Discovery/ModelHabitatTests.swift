import Foundation
import Testing

@testable import HedosKernel

struct ModelHabitatTests {
    @Test func ollamaRootHonorsTheModelsEnvironmentOverride() {
        let habitat = ModelHabitat(
            home: URL(fileURLWithPath: "/Users/someone"),
            environment: ["OLLAMA_MODELS": "/Volumes/big/ollama"])
        let root = habitat.roots(models: ModelsSettings())
            .first { $0.kind == .ollama }?.url.path
        #expect(root == "/Volumes/big/ollama")
    }

    @Test func ollamaRootDefaultsToTheHomeStore() {
        let habitat = ModelHabitat(
            home: URL(fileURLWithPath: "/Users/someone"), environment: [:])
        let root = habitat.roots(models: ModelsSettings())
            .first { $0.kind == .ollama }?.url.path
        #expect(root == "/Users/someone/.ollama/models")
    }

    @Test func ollamaInstallDestinationReportsTheOverriddenStore() async throws {
        let provider = OllamaInstallProvider(
            environment: ["OLLAMA_MODELS": "/Volumes/big/ollama"],
            home: URL(fileURLWithPath: "/Users/someone"))
        let plan = try await provider.plan(reference: "gemma3:4b")
        #expect(plan.destination == "/Volumes/big/ollama")
    }
}
