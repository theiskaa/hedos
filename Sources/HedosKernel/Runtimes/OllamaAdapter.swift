import Foundation

public struct OllamaAdapter: RuntimeAdapter {
    public var id: String { "ollama" }
    public let baseURL: URL

    public init(baseURL: URL = URL(string: "http://127.0.0.1:11434")!) {
        self.baseURL = baseURL
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.source.kind == .ollama && (capability == .chat || capability == .complete)
    }

    public func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
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

    static func requestBody(model: String, payload: JSONValue) throws -> Data {
        guard case .object(let object) = payload, let messages = object["messages"] else {
            throw KernelError.runtimeFailed("chat payload must carry a messages array")
        }
        let body: JSONValue = .object([
            "model": .string(model),
            "messages": messages,
            "stream": .bool(true),
        ])
        return try JSONEncoder().encode(body)
    }
}
