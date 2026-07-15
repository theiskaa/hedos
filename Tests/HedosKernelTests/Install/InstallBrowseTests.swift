import Foundation
import Synchronization
import Testing

@testable import HedosKernel

private final class FakeSearchProvider: InstallProvider, @unchecked Sendable {
    let id = InstallProviderID.huggingface
    let displayName = "Fake Hugging Face"
    let sourceKind = SourceKind.huggingfaceCache
    let supportsSearch = true
    let results: @Sendable (String) throws -> [InstallSearchHit]
    let received = Mutex<[String]>([])

    init(results: @escaping @Sendable (String) throws -> [InstallSearchHit]) {
        self.results = results
    }

    func availability() async -> InstallAvailability { .ready }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        received.withLock { $0.append(query) }
        return try results(query)
    }

    func plan(reference: String) async throws -> InstallPlan {
        InstallPlan(
            provider: id, reference: reference, displayName: reference, destination: "~")
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { $0.finish() }
    }
}

private func hit(_ reference: String, downloads: Int = 1) -> InstallSearchHit {
    InstallSearchHit(
        provider: .huggingface, reference: reference,
        name: reference.split(separator: "/").last.map(String.init) ?? reference,
        downloads: downloads)
}

struct InstallBrowseTests {
    @Test func ollamaDirectReferenceClassifiesShapes() {
        #expect(InstallService.ollamaDirectReference(for: "gemma3:4b") == "gemma3:4b")
        #expect(
            InstallService.ollamaDirectReference(for: "https://ollama.com/library/gemma3")
                == "gemma3")
        #expect(InstallService.ollamaDirectReference(for: "gemma3") == nil)
        #expect(InstallService.ollamaDirectReference(for: "ollama") == nil)
        #expect(InstallService.ollamaDirectReference(for: "smollama") == nil)
        #expect(InstallService.ollamaDirectReference(for: "org/repo") == nil)
        #expect(
            InstallService.ollamaDirectReference(for: "https://huggingface.co/org/repo") == nil)
    }

    @Test func browseSearchesWithTheNormalizedRepo() async {
        let provider = FakeSearchProvider { _ in [hit("org/repo")] }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "https://huggingface.co/org/repo")
        #expect(provider.received.withLock { $0 } == ["org/repo"])
        #expect(result.hits.map(\.reference) == ["org/repo"])
        #expect(result.failureHint == nil)
    }

    @Test func browsePinsPastedLinkWhenSearchMissesIt() async {
        let provider = FakeSearchProvider { _ in [hit("other/model")] }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "hf.co/org/repo")
        #expect(result.hits.first?.reference == "org/repo")
        #expect(result.hits.map(\.reference).contains("other/model"))
    }

    @Test func browseDoesNotFabricateAHitForTypedPartialRepos() async {
        let provider = FakeSearchProvider { _ in [hit("TheBloke/Llama-2-7B-GGUF")] }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "TheBloke/L")
        #expect(result.hits.map(\.reference) == ["TheBloke/Llama-2-7B-GGUF"])
    }

    @Test func browsePinsTypedRepoOnlyWhenNothingMatched() async {
        let provider = FakeSearchProvider { _ in [] }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "org/very-new-model")
        #expect(result.hits.map(\.reference) == ["org/very-new-model"])
    }

    @Test func browseKeepsThePastedRepoWhenSearchFails() async {
        let provider = FakeSearchProvider { _ in
            throw InstallError.transferFailed("hugging face search returned HTTP 500")
        }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "https://huggingface.co/org/repo")
        #expect(result.hits.map(\.reference) == ["org/repo"])
        #expect(result.failureHint == nil)
    }

    @Test func browseSurfacesFailureForPlainQueries() async {
        let provider = FakeSearchProvider { _ in
            throw InstallError.transferFailed("hugging face search returned HTTP 500")
        }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "gemma")
        #expect(result.hits.isEmpty)
        #expect(result.failureHint == "hugging face search returned HTTP 500")
    }

    @Test func browseSkipsOllamaShapedQueries() async {
        let provider = FakeSearchProvider { _ in [hit("org/repo")] }
        let service = InstallService(providers: [provider], freeDiskBytes: { _ in .max })
        let result = await service.browse(matching: "gemma3:4b")
        #expect(result.hits.isEmpty)
        #expect(provider.received.withLock { $0 }.isEmpty)
    }
}
