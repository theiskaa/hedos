import Foundation

public struct OpenAIEndpointAdapter: RuntimeAdapter {
    public var id: String { "generic:openai-server" }

    private let secrets: any SecretStore

    public init(secrets: any SecretStore = KeychainStore()) {
        self.secrets = secrets
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
        guard let url = URL(string: normalizedBase(base)), let host = url.host else {
            return normalizedBase(base)
        }
        let port = url.port.map { ":\($0)" } ?? ""
        return "\(host)\(port)"
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && (capability == .chat || capability == .complete)
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .endpoint else { return nil }
        return RuntimeBid(tier: .native, preference: 10)
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let secrets = secrets
            let task = Task {
                let base = Self.normalizedBase(record.source.path)
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
                        throw KernelError.runtimeUnavailable(
                            hint:
                                "The server refused the API key. Update it under Settings → Models → Servers."
                        )
                    }
                    guard http.statusCode == 200 else {
                        throw KernelError.runtimeFailed(
                            "\(base) returned HTTP \(http.statusCode)")
                    }
                    var parser = OpenAIStreamParser()
                    for try await line in bytes.lines {
                        if Task.isCancelled { break }
                        for chunk in parser.parse(line: line) {
                            continuation.yield(chunk)
                            if case .done = chunk {
                                continuation.finish()
                                return
                            }
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch let error as URLError
                    where error.code == .cannotConnectToHost || error.code == .cannotFindHost
                    || error.code == .networkConnectionLost
                {
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
