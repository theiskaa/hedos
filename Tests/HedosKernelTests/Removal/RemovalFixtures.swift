import Foundation
import Synchronization
import Testing

@testable import HedosKernel

final class FakeTrasher: @unchecked Sendable {
    let bin: URL
    private let state = Mutex<(trashed: [String], failSuffix: String?)>(([], nil))

    init(bin: URL) {
        self.bin = bin
        try? FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
    }

    var trashed: [String] {
        state.withLock { $0.trashed }
    }

    func failOnPathSuffix(_ suffix: String?) {
        state.withLock { $0.failSuffix = suffix }
    }

    func callAsFunction(_ url: URL) throws {
        let failSuffix = state.withLock { $0.failSuffix }
        if let failSuffix, url.path.hasSuffix(failSuffix) {
            throw RemovalError.trashFailed(path: url.path, reason: "refused by test")
        }
        let destination = bin.appendingPathComponent(
            "\(UUID().uuidString)-\(url.lastPathComponent)")
        try FileManager.default.moveItem(at: url, to: destination)
        state.withLock { $0.trashed.append(url.path) }
    }
}

final class FakeOllamaTransport: InstallTransport, @unchecked Sendable {
    private let state = Mutex<State>(State())

    private struct State {
        var reachable = true
        var deleteStatus = 200
        var deleteBody = Data("{}".utf8)
        var requests: [URLRequest] = []
        var onDelete: (@Sendable () -> Void)?
        var deleteError: (any Error)?
    }

    var recordedRequests: [URLRequest] {
        state.withLock { $0.requests }
    }

    var deleteRequests: [URLRequest] {
        recordedRequests.filter { $0.url?.path.hasSuffix("api/delete") == true }
    }

    func setReachable(_ reachable: Bool) {
        state.withLock { $0.reachable = reachable }
    }

    func setDeleteResponse(status: Int, body: String = "{}") {
        state.withLock {
            $0.deleteStatus = status
            $0.deleteBody = Data(body.utf8)
        }
    }

    func onDelete(_ effect: @escaping @Sendable () -> Void) {
        state.withLock { $0.onDelete = effect }
    }

    func failDelete(_ error: any Error) {
        state.withLock { $0.deleteError = error }
    }

    func fetch(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        state.withLock { $0.requests.append(request) }
        guard let url = request.url else { throw URLError(.badURL) }
        if url.path.hasSuffix("api/tags") {
            guard state.withLock({ $0.reachable }) else {
                throw URLError(.cannotConnectToHost)
            }
            return (Data("{}".utf8), response(url: url, status: 200))
        }
        if url.path.hasSuffix("api/delete") {
            let (status, body, effect, thrown) = state.withLock {
                ($0.deleteStatus, $0.deleteBody, $0.onDelete, $0.deleteError)
            }
            if let thrown { throw thrown }
            if status == 200 {
                effect?()
            }
            return (body, response(url: url, status: status))
        }
        return (Data(), response(url: url, status: 404))
    }

    func stream(_ request: URLRequest) async throws -> (
        AsyncThrowingStream<Data, Error>, HTTPURLResponse
    ) {
        guard let url = request.url else { throw URLError(.badURL) }
        return (
            AsyncThrowingStream { $0.finish() },
            response(url: url, status: 404)
        )
    }

    private func response(url: URL, status: Int) -> HTTPURLResponse {
        HTTPURLResponse(url: url, statusCode: status, httpVersion: nil, headerFields: nil)!
    }
}

final class HeldInstallProvider: InstallProvider, @unchecked Sendable {
    let id: InstallProviderID
    let displayName = "Held"
    let sourceKind: SourceKind
    let supportsSearch = false
    private let continuation =
        Mutex<AsyncThrowingStream<InstallStreamEvent, Error>.Continuation?>(nil)

    init(id: InstallProviderID, sourceKind: SourceKind) {
        self.id = id
        self.sourceKind = sourceKind
    }

    func availability() async -> InstallAvailability { .ready }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        []
    }

    func plan(reference: String) async throws -> InstallPlan {
        InstallPlan(
            provider: id, reference: reference, displayName: reference, destination: "~")
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { cont in
            cont.yield(.progress(InstallProgress(bytesDownloaded: 1)))
            continuation.withLock { $0 = cont }
        }
    }

    func release() {
        continuation.withLock { $0 }?.finish()
    }
}

enum RemovalFixtures {
    static func tempDirectory(_ label: String) throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("hedos-removal-\(label)-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    static func makeKernel(
        home: URL, directory: URL, trasher: FakeTrasher,
        transport: FakeOllamaTransport = FakeOllamaTransport(),
        binaryPresent: Bool = false,
        startDaemon: (@Sendable () async throws -> Void)? = nil,
        installProviders: [any InstallProvider]? = nil
    ) async -> Kernel {
        let habitat = ModelHabitat(
            home: home, environment: ["HF_HOME": "", "HF_HUB_CACHE": ""])
        let kernel = Kernel(
            directory: directory, governor: MemoryGovernor(totalMemoryMB: 262_144),
            secrets: InMemorySecretStore(), habitat: habitat,
            installProviders: installProviders)
        await kernel.setModelRemover(
            ModelRemover(
                trasher: trasher.callAsFunction,
                ollama: OllamaModelRemover(
                    transport: transport,
                    binaryPresent: { binaryPresent },
                    startDaemon: startDaemon ?? {})))
        return kernel
    }

    static func makeHFKernel(
        home: URL, directory: URL, hubRoot: URL, trasher: FakeTrasher
    ) async -> Kernel {
        let habitat = ModelHabitat(
            home: home, environment: ["HF_HOME": "", "HF_HUB_CACHE": hubRoot.path])
        let kernel = Kernel(
            directory: directory, governor: MemoryGovernor(totalMemoryMB: 262_144),
            secrets: InMemorySecretStore(), habitat: habitat)
        await kernel.setModelRemover(
            ModelRemover(
                trasher: trasher.callAsFunction,
                ollama: OllamaModelRemover(
                    transport: FakeOllamaTransport(),
                    binaryPresent: { false },
                    startDaemon: {})))
        return kernel
    }

    static func canon(_ path: String) -> String {
        path.hasPrefix("/private/") ? String(path.dropFirst("/private".count)) : path
    }

    static func canon(_ paths: [String]) -> Set<String> {
        Set(paths.map(canon))
    }

    static func onlyRecord(_ kernel: Kernel, kind: SourceKind) async throws -> ModelRecord {
        let matching = try await kernel.shelf().filter { $0.source.kind == kind }
        #expect(matching.count == 1)
        guard let record = matching.first else {
            throw CocoaError(.fileNoSuchFile)
        }
        return record
    }
}
