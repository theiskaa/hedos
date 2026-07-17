import Foundation

extension Kernel: GatewayPort {
    public func artifactData(id: String) async throws -> Data? {
        guard let url = try await artifactStore.url(id: id) else { return nil }
        return try await Task.detached { try Data(contentsOf: url) }.value
    }

    public func job(id: String) async throws -> Job? {
        try await scheduler.job(id: id)
    }

    public func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        await scheduler.events(id: id)
    }

    public func cancel(jobID: String) async {
        await scheduler.cancel(jobID)
    }

    public func admissionState(
        modelID: String, footprintMB: Int?, kind: GatewayWorkKind
    ) async -> GatewayAdmissionState {
        switch kind {
        case .stream:
            if await governor.wouldWait(admitting: modelID, footprintMB: footprintMB) {
                return .saturated(retryAfterSeconds: GatewayDefaults.saturatedRetryAfterSeconds)
            }
        case .job:
            if await scheduler.queueDepth() >= GatewayDefaults.inferenceQueueDepthCap {
                return .saturated(retryAfterSeconds: GatewayDefaults.queuedRetryAfterSeconds)
            }
        }
        return .ready
    }
}

extension Kernel {
    @discardableResult
    public func startGateway(portOverride: Int? = nil) async throws -> GatewayStatus {
        if let inFlight = gatewayStartTask {
            return try await inFlight.value
        }
        if let gateway, await gateway.status.running {
            return await gateway.status
        }
        if let inFlight = gatewayStartTask {
            return try await inFlight.value
        }
        let start = Task {
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
        gatewayStartTask = start
        defer { gatewayStartTask = nil }
        return try await start.value
    }

    public func startGatewayIfEnabled() async {
        guard await settings.gateway().enabled else { return }
        _ = try? await startGateway()
    }

    public func stopGateway() async {
        if let inFlight = gatewayStartTask {
            _ = try? await inFlight.value
        }
        await gateway?.stop()
        gateway = nil
    }

    public func gatewayStatus() async -> GatewayStatus {
        if let gateway {
            return await gateway.status
        }
        return GatewayStatus(running: false)
    }
}
