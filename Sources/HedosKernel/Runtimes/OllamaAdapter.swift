import Foundation

public struct OllamaResident: Hashable, Sendable, Decodable {
    public let name: String
    public let size: Int64

    public var sizeMB: Int {
        Int(size >> 20)
    }
}

public struct OllamaAdapter: RuntimeAdapter {
    public var id: String { "ollama" }
    public let baseURL: URL

    public init(baseURL: URL = URL(string: "http://127.0.0.1:11434")!) {
        self.baseURL = baseURL
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete || capability == .embed else {
            return false
        }
        if let runtimeID = record.runtime.id { return runtimeID == id }
        return record.source.kind == .ollama
    }

    public func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .ollamaStore else { return nil }
        return RuntimeBid(tier: .native, preference: 20)
    }

    public static func daemonBinary() -> URL? {
        let candidates = [
            "/usr/local/bin/ollama",
            "/opt/homebrew/bin/ollama",
            "\(NSHomeDirectory())/.local/bin/ollama",
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }

    public func loadedModels() async -> [OllamaResident] {
        var request = URLRequest(url: baseURL.appendingPathComponent("api/ps"))
        request.timeoutInterval = 2
        guard let (data, _) = try? await URLSession.shared.data(for: request) else {
            return []
        }
        return Self.parseLoadedModels(data)
    }

    public static func parseLoadedModels(_ data: Data) -> [OllamaResident] {
        struct Payload: Decodable {
            let models: [OllamaResident]
        }
        return (try? JSONDecoder().decode(Payload.self, from: data))?.models ?? []
    }

    public func startDaemon() async throws {
        guard let binary = Self.daemonBinary() else {
            throw KernelError.runtimeUnavailable(
                hint: "Ollama isn't installed. Get it from ollama.com.")
        }
        let process = Process()
        process.executableURL = binary
        process.arguments = ["serve"]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()

        let probe = baseURL.appendingPathComponent("api/tags")
        for _ in 0..<20 {
            try await Task.sleep(nanoseconds: 500_000_000)
            var request = URLRequest(url: probe)
            request.timeoutInterval = 2
            if let (_, response) = try? await URLSession.shared.data(for: request),
                (response as? HTTPURLResponse)?.statusCode == 200
            {
                return
            }
        }
        throw KernelError.runtimeUnavailable(
            hint: "Started Ollama but it never became reachable.")
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        capability == .embed
            ? invokeEmbed(record, payload: payload)
            : invokeChat(record, payload: payload)
    }

    private func invokeChat(
        _ record: ModelRecord, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = URLRequest(url: baseURL.appendingPathComponent("api/chat"))
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    request.httpBody = try Self.requestBody(model: record.name, payload: payload)

                    let (bytes, response) = try await URLSession.shared.bytes(for: request)
                    guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                        let code = (response as? HTTPURLResponse)?.statusCode ?? -1
                        throw KernelError.runtimeFailed("ollama returned HTTP \(code)")
                    }
                    for try await line in bytes.lines {
                        if Task.isCancelled { break }
                        guard let chunk = OllamaStreamParser.parse(line: line) else { continue }
                        continuation.yield(chunk)
                        if case .done = chunk { break }
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
                            hint: "Ollama isn't running. Start it with `ollama serve`."))
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func invokeEmbed(
        _ record: ModelRecord, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var request = URLRequest(url: baseURL.appendingPathComponent("api/embed"))
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    request.httpBody = try Self.embedRequestBody(
                        model: record.name, payload: payload)

                    let (data, response) = try await URLSession.shared.data(for: request)
                    if (response as? HTTPURLResponse)?.statusCode != 200 {
                        throw KernelError.runtimeFailed(
                            Self.embedErrorMessage(
                                data, statusCode: (response as? HTTPURLResponse)?.statusCode ?? -1)
                        )
                    }
                    let parsed = try Self.parseEmbedResponse(data)
                    for vector in parsed.vectors {
                        if Task.isCancelled {
                            continuation.finish(throwing: CancellationError())
                            return
                        }
                        continuation.yield(.vector(vector))
                    }
                    continuation.yield(.done(parsed.promptTokens.map { GenerationStats(promptTokens: $0) }))
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch let error as URLError
                    where error.code == .cannotConnectToHost || error.code == .cannotFindHost
                    || error.code == .networkConnectionLost
                {
                    continuation.finish(
                        throwing: KernelError.runtimeUnavailable(
                            hint: "Ollama isn't running. Start it with `ollama serve`."))
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static let optionKeys: [(payload: String, option: String)] = [
        ("temperature", "temperature"),
        ("top_p", "top_p"),
        ("max_tokens", "num_predict"),
        ("context_length", "num_ctx"),
    ]

    static func requestBody(model: String, payload: JSONValue) throws -> Data {
        guard case .object(let object) = payload, let messages = object["messages"] else {
            throw KernelError.runtimeFailed("chat payload must carry a messages array")
        }
        var body: [String: JSONValue] = [
            "model": .string(model),
            "messages": messages,
            "stream": .bool(true),
        ]
        var options: [String: JSONValue] = [:]
        for mapping in Self.optionKeys {
            if let value = object[mapping.payload], value != .null {
                options[mapping.option] = value
            }
        }
        if !options.isEmpty { body["options"] = .object(options) }
        if let thinking = object["thinking"], thinking != .null {
            body["think"] = thinking
        }
        return try JSONEncoder().encode(JSONValue.object(body))
    }

    static func embedRequestBody(model: String, payload: JSONValue) throws -> Data {
        guard case .object(let object) = payload, let input = object["input"], input != .null
        else {
            throw KernelError.runtimeFailed("embed payload must carry an input")
        }
        let body: [String: JSONValue] = ["model": .string(model), "input": input]
        return try JSONEncoder().encode(JSONValue.object(body))
    }

    static func parseEmbedResponse(_ data: Data) throws -> (
        vectors: [[Double]], promptTokens: Int?
    ) {
        struct ErrorPayload: Decodable {
            let error: String
        }
        if let errorPayload = try? JSONDecoder().decode(ErrorPayload.self, from: data) {
            throw KernelError.runtimeFailed("ollama: \(errorPayload.error)")
        }
        struct Payload: Decodable {
            let embeddings: [[Double]]
            let promptEvalCount: Int?
            enum CodingKeys: String, CodingKey {
                case embeddings
                case promptEvalCount = "prompt_eval_count"
            }
        }
        guard let payload = try? JSONDecoder().decode(Payload.self, from: data),
            !payload.embeddings.isEmpty
        else {
            throw KernelError.runtimeFailed("ollama embed response was not understood")
        }
        return (payload.embeddings, payload.promptEvalCount)
    }

    static func embedErrorMessage(_ data: Data, statusCode: Int) -> String {
        struct ErrorPayload: Decodable {
            let error: String
        }
        if let errorPayload = try? JSONDecoder().decode(ErrorPayload.self, from: data) {
            return "ollama: \(errorPayload.error)"
        }
        return "ollama embed returned HTTP \(statusCode)"
    }
}
