import Foundation

public struct GatewayRoute: Sendable {
    public var method: String
    public var path: String
    public var handler: any GatewayHandling
    public var inference: Bool
    public var group: String
    public var summary: String
    public var maxBodyBytes: Int?

    public init(
        _ method: String, _ path: String, _ handler: any GatewayHandling,
        inference: Bool = false, group: String = "", summary: String = "",
        maxBodyBytes: Int? = nil
    ) {
        self.method = method.uppercased()
        self.path = path
        self.handler = handler
        self.inference = inference
        self.group = group
        self.summary = summary
        self.maxBodyBytes = maxBodyBytes
    }
}

public struct GatewayRouter: Sendable {
    let port: any GatewayPort
    let routes: [GatewayRoute]
    let auth: GatewayAuth
    let audit: GatewayAuditLog
    let maxConcurrentInference: Int
    private let inflight = GatewayCounter()

    public init(
        port: any GatewayPort, auth: GatewayAuth, audit: GatewayAuditLog,
        routes: [GatewayRoute] = [], maxConcurrentInference: Int = 4
    ) {
        self.port = port
        self.auth = auth
        self.audit = audit
        self.routes = routes
        self.maxConcurrentInference = maxConcurrentInference
    }

    static func surface(for path: String) -> GatewaySurface {
        path.hasPrefix("/api") ? .ollama : .openAI
    }

    func bodyLimit(for uri: String, default defaultLimit: Int) -> Int {
        let path = GatewayRequest(method: "GET", uri: uri, headers: [], body: Data()).path
        return routes.first { $0.path == path }?.maxBodyBytes ?? defaultLimit
    }

    public static func standardRoutes() -> [GatewayRoute] {
        [
            GatewayRoute(
                "POST", "/v1/chat/completions", OpenAIChatHandler(), inference: true,
                group: "OpenAI", summary: "Stream or complete a chat"),
            GatewayRoute(
                "GET", "/v1/models", OpenAIModelsHandler(),
                group: "OpenAI", summary: "List the models this token can reach"),
            GatewayRoute(
                "POST", "/v1/embeddings", OpenAIEmbeddingsHandler(),
                group: "OpenAI", summary: "Embed text (when a model serves it)"),
            GatewayRoute(
                "POST", "/v1/completions", OpenAICompletionsHandler(), inference: true,
                group: "OpenAI", summary: "Prompt-only text completion"),
            GatewayRoute(
                "POST", "/v1/audio/speech", OpenAISpeechHandler(), inference: true,
                group: "OpenAI", summary: "Speak text to WAV audio"),
            GatewayRoute(
                "POST", "/v1/audio/transcriptions", OpenAITranscriptionsHandler(), inference: true,
                group: "OpenAI", summary: "Transcribe an audio file to text",
                maxBodyBytes: 32 * 1024 * 1024),
            GatewayRoute(
                "POST", "/v1/images/generations", OpenAIImagesHandler(), inference: true,
                group: "OpenAI", summary: "Generate an image (base64 PNG)"),
            GatewayRoute(
                "POST", "/api/chat", OllamaChatHandler(), inference: true,
                group: "Ollama", summary: "Chat over the Ollama NDJSON protocol"),
            GatewayRoute(
                "POST", "/api/generate", OllamaGenerateHandler(), inference: true,
                group: "Ollama", summary: "Prompt-only generate, Ollama-style"),
            GatewayRoute(
                "POST", "/api/embed", OllamaEmbedHandler(), inference: true,
                group: "Ollama", summary: "Embed text, Ollama-style"),
            GatewayRoute(
                "POST", "/api/embeddings", OllamaEmbedHandler(), inference: true,
                group: "Ollama", summary: "Embed text (legacy single-vector)"),
            GatewayRoute(
                "GET", "/api/tags", OllamaTagsHandler(),
                group: "Ollama", summary: "List models, Ollama-style"),
            GatewayRoute(
                "GET", "/api/version", OllamaVersionHandler(),
                group: "Ollama", summary: "Version handshake for stock clients"),
            GatewayRoute(
                "POST", "/api/show", OllamaShowHandler(),
                group: "Ollama", summary: "Model details handshake"),
            GatewayRoute(
                "GET", "/v1/pipelines", PipelineListHandler(),
                group: "Pipelines", summary: "List your saved pipelines"),
            GatewayRoute(
                "POST", "/v1/pipelines/run", PipelineRunHandler(), inference: true,
                group: "Pipelines", summary: "Run a saved pipeline by id"),
        ]
    }

    func dispatch(_ request: GatewayRequest, responder: GatewayResponder) async throws {
        let started = Date()
        let surface = Self.surface(for: request.path)
        var identity: GatewayIdentity?
        do {
            identity = try await auth.authenticate(request)
            let outcome = try await route(
                request, identity: identity!, surface: surface, responder: responder)
            await audit.append(
                entry(
                    for: request, identity: identity, outcome: outcome, started: started,
                    detail: nil))
        } catch {
            let gatewayError = GatewayError.wrapping(error)
            let detail = gatewayError.kind == .serverError ? String(describing: error) : nil
            let auditEntry = entry(
                for: request, identity: identity,
                outcome: GatewayOutcome(
                    status: gatewayError.status, outcome: gatewayError.auditOutcome),
                started: started, detail: detail)
            if gatewayError.status == 401, identity == nil {
                await audit.appendUnauthorized(auditEntry)
            } else {
                await audit.append(auditEntry)
            }
            guard !responder.hasStarted else { throw gatewayError }
            try await render(gatewayError, surface: surface, responder: responder)
        }
    }

    private func route(
        _ request: GatewayRequest, identity: GatewayIdentity, surface: GatewaySurface,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        let matching = routes.filter { $0.path == request.path }
        guard !matching.isEmpty else {
            throw GatewayError(.notFound, "no route for \(request.path)")
        }
        guard let route = matching.first(where: { $0.method == request.method }) else {
            throw GatewayError(.methodNotAllowed, "\(request.method) not allowed on \(request.path)")
        }
        if route.inference {
            guard inflight.enter(limit: maxConcurrentInference) else {
                throw GatewayError(
                    .overloaded, "too many requests are already running — retry shortly",
                    retryAfterSeconds: 1)
            }
        }
        defer {
            if route.inference { inflight.exit() }
        }
        return try await route.handler.handle(
            request, identity: identity, port: port, responder: responder)
    }

    private func entry(
        for request: GatewayRequest, identity: GatewayIdentity?, outcome: GatewayOutcome,
        started: Date, detail: String?
    ) -> GatewayAuditEntry {
        GatewayAuditEntry(
            client: identity?.clientID,
            clientName: identity?.name,
            method: request.method,
            route: request.path,
            model: outcome.model,
            capability: outcome.capability,
            outcome: outcome.outcome,
            status: outcome.status,
            durationMs: Int(Date().timeIntervalSince(started) * 1000),
            detail: detail)
    }

    func render(
        _ error: GatewayError, surface: GatewaySurface, responder: GatewayResponder
    ) async throws {
        var extraHeaders: [(String, String)] = []
        if let retryAfter = error.retryAfterSeconds {
            extraHeaders.append(("Retry-After", String(retryAfter)))
        }
        try await responder.respond(
            status: error.status, body: error.body(for: surface), extraHeaders: extraHeaders)
    }
}
