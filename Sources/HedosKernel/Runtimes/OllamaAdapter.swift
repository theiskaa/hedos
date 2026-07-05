import Foundation

public struct OllamaAdapter: RuntimeAdapter {
    public var id: String { "ollama" }
    public let baseURL: URL

    public init(baseURL: URL = URL(string: "http://127.0.0.1:11434")!) {
        self.baseURL = baseURL
    }

    public func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        guard capability == .chat || capability == .complete else { return false }
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
