import Foundation

final class OpenAIEndpointConcurrencyGate: @unchecked Sendable {
    static let shared = OpenAIEndpointConcurrencyGate()

    private let lock = NSLock()
    private var counts: [String: Int] = [:]
    let limit: Int

    init(limit: Int = 4) {
        self.limit = limit
    }

    func acquire(_ base: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let count = counts[base, default: 0]
        guard count < limit else { return false }
        counts[base] = count + 1
        return true
    }

    func release(_ base: String) {
        lock.lock()
        defer { lock.unlock() }
        guard let count = counts[base] else { return }
        if count <= 1 {
            counts.removeValue(forKey: base)
        } else {
            counts[base] = count - 1
        }
    }
}

public struct OpenAIEndpointAdapter: RuntimeAdapter {
    public var id: String { "generic:openai-server" }

    static let defaultMaxResponseBytes = 32 * 1024 * 1024
    static let defaultMaxLineBytes = 2 * 1024 * 1024

    private let secrets: any SecretStore
    private let registry: Registry?
    private let concurrencyGate: OpenAIEndpointConcurrencyGate
    let maxResponseBytes: Int
    let maxLineBytes: Int

    public init(
        secrets: any SecretStore = KeychainStore(), registry: Registry? = nil
    ) {
        self.secrets = secrets
        self.registry = registry
        self.concurrencyGate = .shared
        self.maxResponseBytes = Self.defaultMaxResponseBytes
        self.maxLineBytes = Self.defaultMaxLineBytes
    }

    init(
        secrets: any SecretStore, registry: Registry?,
        concurrencyGate: OpenAIEndpointConcurrencyGate,
        maxResponseBytes: Int = OpenAIEndpointAdapter.defaultMaxResponseBytes,
        maxLineBytes: Int = OpenAIEndpointAdapter.defaultMaxLineBytes
    ) {
        self.secrets = secrets
        self.registry = registry
        self.concurrencyGate = concurrencyGate
        self.maxResponseBytes = maxResponseBytes
        self.maxLineBytes = maxLineBytes
    }

    public static func normalizedBase(_ raw: String) -> String {
        var base = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if !base.contains("://") {
            base = "http://\(base)"
        }
        while base.hasSuffix("/") {
            base = String(base.dropLast())
        }
        if base.lowercased().hasSuffix("/v1") {
            base = String(base.dropLast(3))
            while base.hasSuffix("/") {
                base = String(base.dropLast())
            }
        }
        return base
    }

    public static func account(for base: String) -> String {
        normalizedBase(base)
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .endpoint else { return nil }
        return RuntimeBid(tier: .remote, preference: 10)
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let secrets = secrets
            let registry = registry
            let recordID = record.id
            let gate = concurrencyGate
            let maxResponseBytes = maxResponseBytes
            let maxLineBytes = maxLineBytes
            let task = Task {
                let base = Self.normalizedBase(record.source.path)
                guard gate.acquire(base) else {
                    continuation.finish(
                        throwing: KernelError.runtimeUnavailable(
                            hint:
                                "Too many requests are already in flight to \(base). Retry shortly."
                        ))
                    return
                }
                defer { gate.release(base) }
                do {
                    guard let url = URL(string: "\(base)/v1/chat/completions") else {
                        throw KernelError.runtimeFailed("\(base) is not a valid server URL")
                    }
                    var request = URLRequest(url: url)
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    if let key = try? secrets.get(account: Self.account(for: base)), !key.isEmpty {
                        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
                    }
                    request.httpBody = try Self.requestBody(
                        model: record.source.repo ?? record.name, payload: payload)

                    let (bytes, response) = try await URLSession.shared.bytes(for: request)
                    guard let http = response as? HTTPURLResponse else {
                        throw KernelError.runtimeFailed("\(base) returned no HTTP response")
                    }
                    if http.statusCode == 401 || http.statusCode == 403 {
                        await Self.markUnreachable(recordID, registry: registry)
                        throw KernelError.runtimeUnavailable(
                            hint:
                                "The server refused the API key. Update it under Settings → Models → Servers."
                        )
                    }
                    guard http.statusCode == 200 else {
                        throw KernelError.runtimeUnavailable(
                            hint:
                                "The server answered with HTTP \(http.statusCode). Check that it's serving the OpenAI API and the model is available."
                        )
                    }
                    await Self.markReachable(recordID, registry: registry)
                    var parser = OpenAIStreamParser()
                    var lineBuffer: [UInt8] = []
                    var totalBytes = 0
                    for try await byte in bytes {
                        if Task.isCancelled { break }
                        totalBytes += 1
                        if totalBytes > maxResponseBytes {
                            throw KernelError.runtimeFailed(
                                "\(base) sent a response larger than \(maxResponseBytes) bytes"
                            )
                        }
                        if byte == 0x0A {
                            let line = String(decoding: lineBuffer, as: UTF8.self)
                            lineBuffer.removeAll(keepingCapacity: true)
                            var finished = false
                            for chunk in parser.parse(line: line) {
                                continuation.yield(chunk)
                                if case .done = chunk {
                                    finished = true
                                }
                            }
                            if finished {
                                continuation.finish()
                                return
                            }
                            continue
                        }
                        lineBuffer.append(byte)
                        if lineBuffer.count > maxLineBytes {
                            throw KernelError.runtimeFailed(
                                "\(base) sent a line larger than \(maxLineBytes) bytes")
                        }
                    }
                    continuation.finish()
                } catch where Task.isCancelled {
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch let error as URLError where error.code == .cancelled {
                    continuation.finish()
                } catch let error as URLError where Self.isConnectionFailure(error) {
                    await Self.markUnreachable(recordID, registry: registry)
                    continuation.finish(
                        throwing: KernelError.runtimeUnavailable(
                            hint: "The server at \(base) isn't reachable."))
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func isConnectionFailure(_ error: URLError) -> Bool {
        switch error.code {
        case .cannotConnectToHost, .cannotFindHost, .networkConnectionLost, .timedOut,
            .dnsLookupFailed, .notConnectedToInternet:
            return true
        default:
            return false
        }
    }

    private static func markUnreachable(_ id: String, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == .endpoint,
            record.state != .missing
        else { return }
        try? await registry.setStateIfPresent(id: id, to: .missing)
    }

    private static func markReachable(_ id: String, registry: Registry?) async {
        guard let registry else { return }
        guard let record = try? await registry.get(id: id), record.source.kind == .endpoint,
            record.state != .ready
        else { return }
        try? await registry.setStateIfPresent(id: id, to: .ready)
    }

    static let optionKeys = ["temperature", "top_p", "max_tokens"]

    static func requestBody(model: String, payload: JSONValue) throws -> Data {
        guard case .object(let object) = payload else {
            throw KernelError.runtimeFailed("chat payload must be an object")
        }
        var body: [String: JSONValue] = [
            "model": .string(model),
            "stream": .bool(true),
            "stream_options": .object(["include_usage": .bool(true)]),
        ]
        if let messages = object["messages"] {
            body["messages"] = messages
        } else if case .string(let prompt)? = object["prompt"] {
            body["messages"] = .array([
                .object(["role": .string("user"), "content": .string(prompt)])
            ])
        } else {
            throw KernelError.runtimeFailed("chat payload must carry a messages array")
        }
        for key in optionKeys {
            if let value = object[key], value != .null {
                body[key] = value
            }
        }
        return try JSONEncoder().encode(JSONValue.object(body))
    }

    public static func listModels(baseURL: String, key: String?) async throws -> [String] {
        let base = normalizedBase(baseURL)
        guard let url = URL(string: "\(base)/v1/models") else {
            throw KernelError.runtimeFailed("\(base) is not a valid server URL")
        }
        var request = URLRequest(url: url)
        if let key, !key.isEmpty {
            request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        }
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                throw KernelError.runtimeFailed("\(base) returned no HTTP response")
            }
            if http.statusCode == 401 || http.statusCode == 403 {
                throw KernelError.runtimeUnavailable(
                    hint: "The server refused the API key.")
            }
            guard http.statusCode == 200 else {
                throw KernelError.runtimeFailed("\(base) returned HTTP \(http.statusCode)")
            }
            guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                let entries = object["data"] as? [[String: Any]]
            else {
                throw KernelError.runtimeFailed(
                    "\(base) does not look like an OpenAI-compatible server")
            }
            return entries.compactMap { $0["id"] as? String }.sorted()
        } catch let error as URLError
            where error.code == .cannotConnectToHost || error.code == .cannotFindHost
            || error.code == .networkConnectionLost
        {
            throw KernelError.runtimeUnavailable(
                hint: "The server at \(base) isn't reachable.")
        }
    }
}
