import Foundation

struct OllamaResident: Hashable, Sendable, Decodable {
    let name: String
    let size: Int64

    var sizeMB: Int {
        Int(size >> 20)
    }
}

struct OllamaAdapter: RuntimeAdapter {
    var id: RuntimeID { .ollama }
    let baseURL: URL

    init(baseURL: URL = URL(string: "http://127.0.0.1:11434")!) {
        self.baseURL = baseURL
    }

    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        requested ?? record.contextLength
    }

    func supportsTools(_ record: ModelRecord) -> Bool {
        true
    }

    func honoredParamKeys(_ record: ModelRecord, _ capability: Capability) -> Set<String> {
        guard capability == .chat || capability == .complete else { return [] }
        return [
            "temperature", "top_p", "top_k", "min_p", "max_tokens", "context_length",
            "stop", "seed", "repeat_penalty", "frequency_penalty", "presence_penalty",
            "response_format",
        ]
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete || capability == .embed
            || capability == .see
        else {
            return false
        }
        if capability == .see, !record.capabilities.contains(.see) {
            return false
        }
        if capability != .embed, !record.capabilities.contains(.chat) {
            return false
        }
        if let runtimeID = record.runtime.id { return runtimeID == id }
        return record.source.kind == .ollama
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .ollamaStore else { return nil }
        return RuntimeBid(tier: .native, preference: BidPreference.ollama)
    }

    static func daemonBinary(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> URL? {
        var candidates = [
            "/usr/local/bin/ollama",
            "/opt/homebrew/bin/ollama",
            "\(NSHomeDirectory())/.local/bin/ollama",
        ]
        if let path = environment["PATH"] {
            candidates.append(
                contentsOf: path.split(separator: ":").map { "\($0)/ollama" })
        }
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { URL(fileURLWithPath: $0) }
    }

    func loadedModels() async -> [OllamaResident] {
        var request = URLRequest(url: baseURL.appendingPathComponent("api/ps"))
        request.timeoutInterval = 2
        guard let (data, _) = try? await URLSession.shared.data(for: request) else {
            return []
        }
        return Self.parseLoadedModels(data)
    }

    static func parseLoadedModels(_ data: Data) -> [OllamaResident] {
        struct Payload: Decodable {
            let models: [OllamaResident]
        }
        return (try? JSONDecoder().decode(Payload.self, from: data))?.models ?? []
    }

    func startDaemon(session: URLSession = .shared) async throws {
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
        var exited = false
        for _ in 0..<20 {
            try await Task.sleep(nanoseconds: 500_000_000)
            var request = URLRequest(url: probe)
            request.timeoutInterval = 2
            if let (_, response) = try? await session.data(for: request),
                (response as? HTTPURLResponse)?.statusCode == 200
            {
                return
            }
            if !process.isRunning {
                exited = true
            }
        }
        throw KernelError.runtimeUnavailable(
            hint: exited
                ? "Ollama quit as soon as it started. Try `ollama serve` in a terminal."
                : "Started Ollama but it never became reachable.")
    }

    func invoke(
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
                        var collected = Data()
                        for try await byte in bytes {
                            collected.append(byte)
                            if collected.count >= 65536 { break }
                        }
                        if let message = Self.errorMessage(fromBody: collected) {
                            throw KernelError.runtimeFailed("ollama: \(message)")
                        }
                        throw KernelError.runtimeFailed("ollama returned HTTP \(code)")
                    }
                    let reader = CappedLineReader(bytes: bytes, source: "ollama")
                    for try await line in reader.lines() {
                        if Task.isCancelled { break }
                        if let message = OllamaStreamParser.errorMessage(line: line) {
                            throw KernelError.runtimeFailed("ollama: \(message)")
                        }
                        for call in OllamaStreamParser.toolCalls(line: line) {
                            continuation.yield(.toolCall(call))
                        }
                        var reachedDone = false
                        for chunk in OllamaStreamParser.parse(line: line) {
                            continuation.yield(chunk)
                            if case .done = chunk { reachedDone = true }
                        }
                        if reachedDone { break }
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

                    let (bytes, response) = try await URLSession.shared.bytes(for: request)
                    var buffer: [UInt8] = []
                    for try await byte in bytes {
                        buffer.append(byte)
                        if buffer.count > OpenAIEndpointAdapter.defaultMaxResponseBytes {
                            throw KernelError.runtimeFailed(
                                "ollama sent a response larger than \(OpenAIEndpointAdapter.defaultMaxResponseBytes) bytes"
                            )
                        }
                    }
                    let data = Data(buffer)
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
        ("top_k", "top_k"),
        ("min_p", "min_p"),
        ("max_tokens", "num_predict"),
        ("context_length", "num_ctx"),
        ("stop", "stop"),
        ("seed", "seed"),
        ("repeat_penalty", "repeat_penalty"),
        ("frequency_penalty", "frequency_penalty"),
        ("presence_penalty", "presence_penalty"),
    ]

    static func requestBody(model: String, payload: JSONValue) throws -> Data {
        guard case .object(let object) = payload else {
            throw KernelError.payloadInvalid("chat payload must be an object")
        }
        let messages: JSONValue
        if let existing = object["messages"] {
            messages = existing
        } else if case .string(let prompt)? = object["prompt"] {
            messages = .array([
                .object(["role": .string("user"), "content": .string(prompt)])
            ])
        } else {
            throw KernelError.payloadInvalid(
                "chat payload must carry a messages array or a prompt")
        }
        var body: [String: JSONValue] = [
            "model": .string(model),
            "messages": wireMessages(messages),
            "stream": .bool(true),
        ]
        if case .array(let tools) = object["tools"] ?? .null, !tools.isEmpty {
            body["tools"] = .array(
                tools.map { tool in
                    .object([
                        "type": .string("function"),
                        "function": tool,
                    ])
                })
        }
        var options: [String: JSONValue] = [:]
        for mapping in Self.optionKeys {
            if let value = object[mapping.payload], value != .null {
                options[mapping.option] = value
            }
        }
        if !options.isEmpty { body["options"] = .object(options) }
        if case .object(let responseFormat)? = object["response_format"],
            let type = responseFormat["type"]?.stringValue
        {
            if type == "json_object" {
                body["format"] = .string("json")
            } else if type == "json_schema",
                case .object(let wrapper)? = responseFormat["json_schema"],
                let schema = wrapper["schema"]
            {
                body["format"] = schema
            }
        }
        if let thinking = object["thinking"], thinking != .null {
            body["think"] = thinking
        } else if carriesToolContent(object) {
            body["think"] = .bool(false)
        }
        return try JSONEncoder().encode(JSONValue.object(body))
    }

    static func carriesToolContent(_ object: [String: JSONValue]) -> Bool {
        if case .array(let tools)? = object["tools"], !tools.isEmpty { return true }
        guard case .array(let messages)? = object["messages"] else { return false }
        return messages.contains { message in
            guard case .object(let fields) = message else { return false }
            if fields["tool_calls"] != nil { return true }
            return fields["role"]?.stringValue == "tool"
        }
    }

    static func wireMessages(_ messages: JSONValue) -> JSONValue {
        guard case .array(let entries) = messages else { return messages }
        return .array(
            entries.map { entry in
                guard case .object(var fields) = entry else { return entry }
                if case .array(let calls)? = fields["tool_calls"] {
                    fields["tool_calls"] = .array(
                        calls.map { call in
                            guard case .object(let parts) = call else { return call }
                            return .object([
                                "function": .object([
                                    "name": parts["name"] ?? .string(""),
                                    "arguments": parts["arguments"] ?? .object([:]),
                                ])
                            ])
                        })
                }
                fields["tool_call_id"] = nil
                return .object(fields)
            })
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

    static func errorMessage(fromBody data: Data) -> String? {
        guard let text = String(data: data, encoding: .utf8) else { return nil }
        for line in text.split(separator: "\n") {
            if let message = OllamaStreamParser.errorMessage(line: String(line)) {
                return message
            }
        }
        return OllamaStreamParser.errorMessage(line: text)
    }
}
