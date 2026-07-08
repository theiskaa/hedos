import Foundation

public struct GatewayRoute: Sendable {
    public var method: String
    public var path: String
    public var handler: any GatewayHandling
    public var inference: Bool

    public init(
        _ method: String, _ path: String, _ handler: any GatewayHandling,
        inference: Bool = false
    ) {
        self.method = method.uppercased()
        self.path = path
        self.handler = handler
        self.inference = inference
    }
}

public struct GatewayRouter: Sendable {
    let port: any GatewayPort
    let routes: [GatewayRoute]
    let auth: GatewayAuth
    let audit: GatewayAuditLog
    let maxConcurrentInference: Int
    private let inflight = GatewayInflight()

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

    public static func standardRoutes() -> [GatewayRoute] {
        [
            GatewayRoute("POST", "/v1/chat/completions", OpenAIChatHandler(), inference: true),
            GatewayRoute("GET", "/v1/models", OpenAIModelsHandler()),
            GatewayRoute("POST", "/v1/embeddings", OpenAIEmbeddingsHandler()),
            GatewayRoute("POST", "/v1/audio/speech", OpenAISpeechHandler(), inference: true),
            GatewayRoute("POST", "/v1/images/generations", OpenAIImagesHandler(), inference: true),
            GatewayRoute("POST", "/api/chat", OllamaChatHandler(), inference: true),
            GatewayRoute("GET", "/api/tags", OllamaTagsHandler()),
            GatewayRoute("GET", "/api/version", OllamaVersionHandler()),
            GatewayRoute("POST", "/api/show", OllamaShowHandler()),
            GatewayRoute("GET", "/v1/pipelines", PipelineListHandler()),
            GatewayRoute("POST", "/v1/pipelines/run", PipelineRunHandler(), inference: true),
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
                entry(for: request, identity: identity, outcome: outcome, started: started))
        } catch {
            let gatewayError = GatewayError.wrapping(error)
            await audit.append(
                entry(
                    for: request, identity: identity,
                    outcome: GatewayOutcome(
                        status: gatewayError.status, outcome: gatewayError.auditOutcome),
                    started: started))
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
        started: Date
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
            durationMs: Int(Date().timeIntervalSince(started) * 1000))
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
