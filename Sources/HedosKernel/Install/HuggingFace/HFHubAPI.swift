import Foundation

protocol InstallTransport: Sendable {
    func fetch(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
    func stream(_ request: URLRequest) async throws -> (
        AsyncThrowingStream<Data, Error>, HTTPURLResponse
    )
}

struct URLSessionInstallTransport: InstallTransport {
    static let chunkBytes = 4 << 20

    let session: URLSession

    init(session: URLSession = .shared) {
        self.session = session
    }

    func fetch(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw InstallError.transferFailed("the server sent a non-HTTP response")
        }
        return (data, http)
    }

    func stream(_ request: URLRequest) async throws -> (
        AsyncThrowingStream<Data, Error>, HTTPURLResponse
    ) {
        let (bytes, response) = try await session.bytes(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw InstallError.transferFailed("the server sent a non-HTTP response")
        }
        let stream = AsyncThrowingStream<Data, Error> { continuation in
            let task = Task {
                do {
                    var buffer = Data(capacity: Self.chunkBytes)
                    for try await byte in bytes {
                        buffer.append(byte)
                        if buffer.count >= Self.chunkBytes {
                            continuation.yield(buffer)
                            buffer = Data(capacity: Self.chunkBytes)
                        }
                    }
                    if !buffer.isEmpty {
                        continuation.yield(buffer)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
        return (stream, http)
    }
}

struct HFSibling: Hashable, Sendable, Decodable {
    let rfilename: String
    let size: Int64?
    let lfs: LFS?

    struct LFS: Hashable, Sendable, Decodable {
        let oid: String?
        let size: Int64?
    }

    init(rfilename: String, size: Int64? = nil, sha256: String? = nil) {
        self.rfilename = rfilename
        self.size = size
        self.lfs = sha256.map { LFS(oid: $0, size: size) }
    }

    var bytes: Int64? { size ?? lfs?.size }
    var sha256: String? { lfs?.oid }
}

struct HFModelInfo: Hashable, Sendable {
    let repo: String
    let sha: String?
    let gated: Bool
    let siblings: [HFSibling]
}

struct HFHubAPI: Sendable {
    static let defaultBaseURL = URL(string: "https://huggingface.co")!

    let baseURL: URL
    let transport: any InstallTransport
    let token: @Sendable () -> String?

    init(
        baseURL: URL = defaultBaseURL,
        transport: any InstallTransport = URLSessionInstallTransport(),
        token: @escaping @Sendable () -> String? = { nil }
    ) {
        self.baseURL = baseURL
        self.transport = transport
        self.token = token
    }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        var components = URLComponents(
            url: baseURL.appendingPathComponent("api/models"), resolvingAgainstBaseURL: false)!
        components.queryItems = [
            URLQueryItem(name: "search", value: query),
            URLQueryItem(name: "limit", value: String(limit)),
            URLQueryItem(name: "sort", value: "downloads"),
        ]
        let (data, http) = try await transport.fetch(authorized(URLRequest(url: components.url!)))
        guard http.statusCode == 200 else {
            throw InstallError.transferFailed("hugging face search returned HTTP \(http.statusCode)")
        }
        struct Hit: Decodable {
            let id: String
            let downloads: Int?
            let likes: Int?
            let lastModified: String?
        }
        let hits = try JSONDecoder().decode([Hit].self, from: data)
        return hits.map { hit in
            InstallSearchHit(
                provider: .huggingface,
                reference: hit.id,
                name: hit.id.split(separator: "/").last.map(String.init) ?? hit.id,
                downloads: hit.downloads,
                likes: hit.likes,
                updatedAt: hit.lastModified.flatMap(Self.parseDate))
        }
    }

    func modelInfo(repo: String) async throws -> HFModelInfo {
        let url = baseURL
            .appendingPathComponent("api/models")
            .appendingPathComponent(repo)
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "blobs", value: "true")]
        let (data, http) = try await transport.fetch(authorized(URLRequest(url: components.url!)))
        switch http.statusCode {
        case 200:
            break
        case 401, 403:
            throw InstallError.authRequired(repo)
        case 404:
            throw InstallError.referenceNotFound(repo)
        default:
            throw InstallError.transferFailed("hugging face returned HTTP \(http.statusCode)")
        }
        struct Info: Decodable {
            let sha: String?
            let gated: Gated?
            let siblings: [HFSibling]?
        }
        let info = try JSONDecoder().decode(Info.self, from: data)
        return HFModelInfo(
            repo: repo, sha: info.sha, gated: info.gated?.isGated ?? false,
            siblings: info.siblings ?? [])
    }

    func resolveRequest(repo: String, revision: String, path: String) -> URLRequest {
        var url = baseURL.appendingPathComponent(repo).appendingPathComponent("resolve")
        url.appendPathComponent(revision)
        for component in path.split(separator: "/") {
            url.appendPathComponent(String(component))
        }
        return authorized(URLRequest(url: url))
    }

    private func authorized(_ request: URLRequest) -> URLRequest {
        var request = request
        if let token = token(), !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    static func parseDate(_ value: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: value) { return date }
        let plain = ISO8601DateFormatter()
        return plain.date(from: value)
    }

    private struct Gated: Decodable {
        let isGated: Bool

        init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            if let flag = try? container.decode(Bool.self) {
                isGated = flag
            } else if let mode = try? container.decode(String.self) {
                isGated = mode.lowercased() != "false"
            } else {
                isGated = false
            }
        }
    }
}
