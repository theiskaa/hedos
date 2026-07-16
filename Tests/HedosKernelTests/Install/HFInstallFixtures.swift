import CryptoKit
import Foundation
import Synchronization

@testable import HedosKernel

func sha256Hex(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

final class FakeHubTransport: InstallTransport, @unchecked Sendable {
    struct Repo {
        var files: [String: Data]
        var revision: String
        var gated: Bool
        var plainPaths: Set<String>

        init(
            files: [String: Data], revision: String = "rev0123abc", gated: Bool = false,
            plainPaths: Set<String> = []
        ) {
            self.files = files
            self.revision = revision
            self.gated = gated
            self.plainPaths = plainPaths
        }
    }

    private let state: Mutex<State>

    private struct State {
        var repos: [String: Repo]
        var honorRange: Bool
        var failingPaths: [String: Int]
        var corruptPaths: Set<String>
        var stallingPaths: Set<String> = []
        var requests: [URLRequest] = []
        var searchHits: Data = Data("[]".utf8)
    }

    init(repos: [String: Repo], honorRange: Bool = true) {
        state = Mutex(
            State(repos: repos, honorRange: honorRange, failingPaths: [:], corruptPaths: []))
    }

    var recordedRequests: [URLRequest] {
        state.withLock { $0.requests }
    }

    func failPath(_ path: String, afterBytes: Int = 0) {
        state.withLock { $0.failingPaths[path] = afterBytes }
    }

    func healPath(_ path: String) {
        state.withLock { _ = $0.failingPaths.removeValue(forKey: path) }
    }

    func corruptPath(_ path: String) {
        state.withLock { _ = $0.corruptPaths.insert(path) }
    }

    func stallPath(_ path: String) {
        state.withLock { _ = $0.stallingPaths.insert(path) }
    }

    func setSearchHits(_ hits: [[String: Any]]) {
        let encoded = (try? JSONSerialization.data(withJSONObject: hits)) ?? Data("[]".utf8)
        state.withLock { $0.searchHits = encoded }
    }

    func fetch(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        record(request)
        guard let url = request.url else { throw URLError(.badURL) }
        let path = url.path
        if path.hasPrefix("/api/models/") {
            let repoName = String(path.dropFirst("/api/models/".count))
            let match = state.withLock { state in
                state.repos.first { $0.key.lowercased() == repoName.lowercased() }
            }
            guard let (canonicalName, repo) = match else {
                return (Data(), response(url: url, status: 404))
            }
            if repo.gated, request.value(forHTTPHeaderField: "Authorization") == nil {
                return (Data(), response(url: url, status: 401))
            }
            let siblings = repo.files.map { path, data in
                var sibling: [String: Any] = ["rfilename": path, "size": data.count]
                if !repo.plainPaths.contains(path) {
                    sibling["lfs"] = ["oid": sha256Hex(data), "size": data.count]
                }
                return sibling
            }
            let body = try JSONSerialization.data(withJSONObject: [
                "id": canonicalName,
                "sha": repo.revision,
                "gated": repo.gated,
                "siblings": siblings,
            ])
            return (body, response(url: url, status: 200))
        }
        if path == "/api/models" {
            let body = state.withLock { $0.searchHits }
            return (body, response(url: url, status: 200))
        }
        return (Data(), response(url: url, status: 404))
    }

    func stream(_ request: URLRequest) async throws -> (
        AsyncThrowingStream<Data, Error>, HTTPURLResponse
    ) {
        record(request)
        guard let url = request.url else { throw URLError(.badURL) }
        let components = url.path.split(separator: "/").map(String.init)
        guard components.count >= 5, components[2] == "resolve" else {
            return (finished([]), response(url: url, status: 404))
        }
        let repoName = components[0] + "/" + components[1]
        let filePath = components[4...].joined(separator: "/")
        let (repo, failAfter, corrupt, stalling, honorRange) = state.withLock {
            (
                $0.repos[repoName], $0.failingPaths[filePath],
                $0.corruptPaths.contains(filePath), $0.stallingPaths.contains(filePath),
                $0.honorRange
            )
        }
        guard let repo, var content = repo.files[filePath] else {
            return (finished([]), response(url: url, status: 404))
        }
        if stalling {
            return (
                AsyncThrowingStream { continuation in
                    let task = Task {
                        do {
                            try await Task.sleep(for: .seconds(300))
                            continuation.finish()
                        } catch {
                            continuation.finish(throwing: error)
                        }
                    }
                    continuation.onTermination = { _ in task.cancel() }
                },
                response(url: url, status: 200)
            )
        }
        if let failAfter, failAfter <= 0 {
            throw URLError(.networkConnectionLost)
        }
        if corrupt {
            content = Data(content.map { $0 ^ 0xFF })
        }
        if honorRange, let range = request.value(forHTTPHeaderField: "Range"),
            range.hasPrefix("bytes="), range.hasSuffix("-"),
            let offset = Int(range.dropFirst("bytes=".count).dropLast()),
            offset > 0
        {
            guard offset < content.count else {
                return (finished([]), response(url: url, status: 416))
            }
            return (
                finished(
                    [Data(content.dropFirst(offset))],
                    failAfterBytes: failAfter),
                response(url: url, status: 206)
            )
        }
        return (
            finished([content], failAfterBytes: failAfter),
            response(url: url, status: 200)
        )
    }

    private func finished(
        _ chunks: [Data], failAfterBytes: Int? = nil
    ) -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            if let failAfterBytes {
                var remaining = failAfterBytes
                for chunk in chunks where !chunk.isEmpty {
                    if chunk.count <= remaining {
                        continuation.yield(chunk)
                        remaining -= chunk.count
                    } else {
                        continuation.yield(chunk.prefix(remaining))
                        continuation.finish(throwing: URLError(.networkConnectionLost))
                        return
                    }
                }
                continuation.finish(throwing: URLError(.networkConnectionLost))
                return
            }
            for chunk in chunks where !chunk.isEmpty {
                continuation.yield(chunk)
            }
            continuation.finish()
        }
    }

    private func record(_ request: URLRequest) {
        state.withLock { $0.requests.append(request) }
    }

    private func response(url: URL, status: Int) -> HTTPURLResponse {
        HTTPURLResponse(url: url, statusCode: status, httpVersion: nil, headerFields: nil)!
    }
}

enum HFInstallFixtures {
    static func tinyRepoFiles(weightBytes: Int = 64) -> [String: Data] {
        [
            "config.json": Data(
                #"{"architectures":["LlamaForCausalLM"],"max_position_embeddings":2048}"#.utf8),
            "tokenizer.json": Data(#"{"version":"1.0"}"#.utf8),
            "model.safetensors": Data(repeating: 0xAB, count: weightBytes),
        ]
    }

    static func provider(
        repos: [String: FakeHubTransport.Repo],
        root: URL,
        token: String? = nil,
        honorRange: Bool = true
    ) -> (HuggingFaceInstallProvider, FakeHubTransport) {
        let transport = FakeHubTransport(repos: repos, honorRange: honorRange)
        let provider = HuggingFaceInstallProvider(
            root: root, transport: transport, tokenProvider: { token })
        return (provider, transport)
    }
}
