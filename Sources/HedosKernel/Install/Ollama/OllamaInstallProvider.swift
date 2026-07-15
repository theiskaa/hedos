import Foundation

struct OllamaInstallProvider: InstallProvider {
    static let pullResponseCap = 256 << 20

    let id = InstallProviderID.ollama
    let displayName = "Ollama"
    let sourceKind = SourceKind.ollama
    let supportsSearch = false

    private let baseURL: URL
    private let session: URLSession

    init(
        baseURL: URL = URL(string: "http://127.0.0.1:11434")!,
        session: URLSession = .shared
    ) {
        self.baseURL = baseURL
        self.session = session
    }

    func availability() async -> InstallAvailability {
        if OllamaAdapter.daemonBinary() != nil {
            return .ready
        }
        if await daemonReachable() {
            return .ready
        }
        return .unavailable(hint: "Ollama isn't installed. Get it from ollama.com.")
    }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        throw InstallError.providerUnavailable(
            hint: "Ollama has no search. Pick from the catalog or enter a tag like gemma3:4b.")
    }

    func plan(reference: String) async throws -> InstallPlan {
        guard let tag = InstallReference.ollamaTag(from: reference) else {
            throw InstallError.referenceInvalid(reference)
        }
        return InstallPlan(
            provider: id, reference: tag, displayName: tag,
            destination: "~/.ollama/models")
    }

    static func isTagShaped(_ reference: String) -> Bool {
        InstallReference.isOllamaTagShaped(reference)
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    if await !daemonReachable() {
                        continuation.yield(.status("Starting Ollama…"))
                        try await OllamaAdapter(baseURL: baseURL).startDaemon()
                    }
                    var request = URLRequest(url: baseURL.appendingPathComponent("api/pull"))
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    request.httpBody = try JSONSerialization.data(
                        withJSONObject: ["model": plan.reference, "stream": true])
                    let (bytes, response) = try await session.bytes(for: request)
                    guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                        let code = (response as? HTTPURLResponse)?.statusCode ?? -1
                        var collected = Data()
                        for try await byte in bytes {
                            collected.append(byte)
                            if collected.count >= 65536 { break }
                        }
                        throw Self.pullFailure(body: collected, code: code)
                    }
                    let reader = CappedLineReader(
                        bytes: bytes, source: "ollama",
                        maxResponseBytes: Self.pullResponseCap)
                    var aggregator = OllamaPullParser.Aggregator()
                    var succeeded = false
                    for try await line in reader.lines() {
                        if Task.isCancelled { break }
                        switch try aggregator.fold(line: line) {
                        case .ignored:
                            break
                        case .status(let message):
                            continuation.yield(.status(message))
                        case .progress(let progress):
                            continuation.yield(.progress(progress))
                        case .success:
                            succeeded = true
                        }
                        if succeeded { break }
                    }
                    if !succeeded, !Task.isCancelled {
                        throw InstallError.transferFailed(
                            "ollama ended the pull without reporting success")
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch let error as URLError
                    where error.code == .cannotConnectToHost || error.code == .cannotFindHost
                    || error.code == .networkConnectionLost
                {
                    continuation.finish(
                        throwing: InstallError.providerUnavailable(
                            hint: "Ollama isn't running. Start it with `ollama serve`."))
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func pullFailure(body: Data, code: Int) -> InstallError {
        struct ErrorBody: Decodable {
            let error: String?
        }
        if let decoded = try? JSONDecoder().decode(ErrorBody.self, from: body),
            let message = decoded.error, !message.isEmpty
        {
            return .transferFailed("ollama: \(message)")
        }
        return .transferFailed("ollama returned HTTP \(code)")
    }

    private func daemonReachable() async -> Bool {
        var request = URLRequest(url: baseURL.appendingPathComponent("api/tags"))
        request.timeoutInterval = 2
        guard let (_, response) = try? await session.data(for: request) else { return false }
        return (response as? HTTPURLResponse)?.statusCode == 200
    }
}
