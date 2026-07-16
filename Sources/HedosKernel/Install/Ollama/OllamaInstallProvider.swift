import Foundation

struct OllamaInstallProvider: InstallProvider {
    static let pullIdleTimeout: TimeInterval = 60 * 60
    static let pullResponseCap = 4 << 30

    let id = InstallProviderID.ollama
    let displayName = "Ollama"
    let sourceKind = SourceKind.ollama
    let supportsSearch = false

    private let baseURL: URL
    private let session: URLSession
    private let environment: [String: String]
    private let home: URL

    init(
        baseURL: URL = OllamaDefaults.baseURL,
        session: URLSession = .shared,
        environment: [String: String] = ProcessInfo.processInfo.environment,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        self.baseURL = baseURL
        self.session = session
        self.environment = environment
        self.home = home
    }

    func availability() async -> InstallAvailability {
        if OllamaAdapter.daemonBinary() != nil {
            return .ready
        }
        if await daemonReachable() {
            return .ready
        }
        return .unavailable(hint: OllamaDefaults.notInstalledHint)
    }

    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit] {
        throw InstallError.providerUnavailable(
            hint: "Ollama has no search. Pick from the catalog or enter a tag like gemma3:4b.")
    }

    func plan(reference: String) async throws -> InstallPlan {
        guard let tag = InstallReference.ollamaInstallTag(from: reference) else {
            throw InstallError.referenceInvalid(reference)
        }
        return InstallPlan(
            provider: id, reference: tag, displayName: tag,
            destination: OllamaDefaults.displayModelsPath(
                environment: environment, home: home))
    }

    static func isTagShaped(_ reference: String) -> Bool {
        InstallReference.isOllamaTagShaped(reference)
    }

    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error> {
        InstallStream.make { error in
            if case KernelError.runtimeUnavailable(let hint) = error {
                return .providerUnavailable(hint: hint)
            }
            if let kernel = error as? KernelError {
                return .transferFailed(kernel.localizedDescription)
            }
            if let url = error as? URLError,
                url.code == .cannotConnectToHost || url.code == .cannotFindHost
                    || url.code == .networkConnectionLost
            {
                return .providerUnavailable(
                    hint: "Ollama isn't running. Start it with `ollama serve`.")
            }
            return nil
        } run: { continuation, _ in
            if await !daemonReachable() {
                try Task.checkCancellation()
                guard OllamaAdapter.daemonBinary() != nil else {
                    throw InstallError.providerUnavailable(
                        hint: "Ollama stopped running. Start it again and retry.")
                }
                continuation.yield(.status("Starting Ollama…"))
                try await OllamaAdapter(baseURL: baseURL).startDaemon(session: session)
            }
            var request = URLRequest(url: baseURL.appendingPathComponent("api/pull"))
            request.httpMethod = "POST"
            request.timeoutInterval = Self.pullIdleTimeout
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
        await OllamaDefaults.daemonReachable(baseURL: baseURL, session: session)
    }
}
