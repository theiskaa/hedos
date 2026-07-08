import Foundation
import Testing

@testable import HedosKernel

private func makeServer(port: Int = 0) throws -> (server: GatewayServer, directory: URL) {
    let directory = try Fixtures.tempDirectory()
    let clients = GatewayClientStore(directory: directory, secrets: InMemorySecretStore())
    let audit = GatewayAuditLog(directory: directory)
    let router = GatewayRouter(port: FakeGatewayPort(), auth: GatewayAuth(clients: clients), audit: audit)
    let server = GatewayServer(configuration: GatewayServer.Configuration(port: port), router: router)
    return (server, directory)
}

@Test func concurrentStartCallsReturnTheSamePort() async throws {
    let (server, directory) = try makeServer()
    defer { try? FileManager.default.removeItem(at: directory) }

    async let a = server.start()
    async let b = server.start()
    let (portA, portB) = try await (a, b)
    #expect(portA == portB)
    #expect(portA > 0)

    await server.stop()
    let restarted = try await server.start()
    #expect(restarted > 0)
    await server.stop()
}

@Test func startAgainstAnAlreadyBoundPortThrowsThenSucceedsOnceFreed() async throws {
    let (occupier, occupierDirectory) = try makeServer()
    defer { try? FileManager.default.removeItem(at: occupierDirectory) }
    let occupiedPort = try await occupier.start()

    let (contender, contenderDirectory) = try makeServer(port: occupiedPort)
    defer { try? FileManager.default.removeItem(at: contenderDirectory) }

    await #expect(throws: (any Error).self) {
        try await contender.start()
    }

    await occupier.stop()

    let freedPort = try await contender.start()
    #expect(freedPort == occupiedPort)
    await contender.stop()
}
