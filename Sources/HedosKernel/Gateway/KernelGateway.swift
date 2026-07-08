import Foundation

extension Kernel: GatewayPort {
    public func artifactData(id: String) async throws -> Data? {
        guard let url = try await artifactURL(id: id) else { return nil }
        return try Data(contentsOf: url)
    }

    public func admissionState(
        modelID: String, footprintMB: Int?, kind: GatewayWorkKind
    ) async -> GatewayAdmissionState {
        switch kind {
        case .stream:
            if await governor.wouldWait(admitting: modelID, footprintMB: footprintMB) {
                return .saturated(retryAfterSeconds: 1)
            }
        case .job:
            if await scheduler.queueDepth() >= 4 {
                return .saturated(retryAfterSeconds: 5)
            }
        }
        return .ready
    }
}

extension Kernel {
    public func gatewaySettings() async -> GatewaySettings {
        await settings.gateway()
    }

    public func updateGatewaySettings(_ value: GatewaySettings) async throws {
        try await settings.save(value)
    }

    @discardableResult
    public func startGateway(portOverride: Int? = nil) async throws -> GatewayStatus {
        if let gateway, await gateway.status.running {
            return await gateway.status
        }
        let stored = await settings.gateway()
        let configuration = GatewayServer.Configuration(
            port: portOverride ?? stored.port,
            maxConnections: stored.maxConnections)
        let router = GatewayRouter(
            port: self,
            auth: GatewayAuth(clients: gatewayClientStore),
            audit: gatewayAuditLog,
            routes: GatewayRouter.standardRoutes(),
            maxConcurrentInference: stored.maxConcurrentInference)
        let server = GatewayServer(configuration: configuration, router: router)
        gateway = server
        do {
            _ = try await server.start()
        } catch {
            gateway = nil
            throw error
        }
        return await server.status
    }

    public func startGatewayIfEnabled() async {
        guard await settings.gateway().enabled else { return }
        _ = try? await startGateway()
    }

    public func stopGateway() async {
        await gateway?.stop()
        gateway = nil
    }

    public func gatewayStatus() async -> GatewayStatus {
        if let gateway {
            return await gateway.status
        }
        return GatewayStatus(running: false)
    }

    public func createGatewayClient(
        name: String, scopes: GatewayScopes
    ) async throws -> GatewayClientCreation {
        try await gatewayClientStore.create(name: name, scopes: scopes)
    }

    public func gatewayClients() async -> [GatewayClient] {
        await gatewayClientStore.list()
    }

    public func revokeGatewayClient(id: String) async throws {
        try await gatewayClientStore.revoke(id: id)
    }

    public func gatewayAudit(limit: Int = 20) async -> [GatewayAuditEntry] {
        await gatewayAuditLog.tail(limit: limit)
    }

    public nonisolated var gatewayAuditURL: URL {
        gatewayAuditLog.logURL
    }
}
