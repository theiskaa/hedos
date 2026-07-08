import Foundation
import Testing

@testable import HedosKernel

@Test func mintedTokenVerifiesAndCarriesScopes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = GatewayClientStore(directory: dir, secrets: InMemorySecretStore())
    let scopes = GatewayScopes(models: ["m1"], capabilities: ["chat"])
    let creation = try await store.create(name: "editor", scopes: scopes)

    #expect(creation.token.hasPrefix("hd_"))
    let identity = await store.verify(token: creation.token)
    #expect(identity?.clientID == creation.client.id)
    #expect(identity?.name == "editor")
    #expect(identity?.scopes == scopes)
}

@Test func secretStoreHoldsHashNotPlaintext() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = InMemorySecretStore()
    let store = GatewayClientStore(directory: dir, secrets: secrets)
    let creation = try await store.create(name: "hashy", scopes: .all)

    let secret = String(creation.token.dropFirst(3).split(separator: ".")[1])
    let stored = try secrets.get(account: GatewayClientStore.secretAccount(creation.client.id))
    #expect(stored != nil)
    #expect(stored != secret)
    #expect(stored == GatewayClientStore.hash(secret))

    let onDisk = try String(
        contentsOf: dir.appendingPathComponent("clients.json"), encoding: .utf8)
    #expect(!onDisk.contains(secret))
    #expect(!onDisk.contains(stored!))
}

@Test func malformedAndUnknownTokensFailVerify() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = GatewayClientStore(directory: dir, secrets: InMemorySecretStore())
    let creation = try await store.create(name: "real", scopes: .all)

    #expect(await store.verify(token: "") == nil)
    #expect(await store.verify(token: "not-a-token") == nil)
    #expect(await store.verify(token: "hd_nodot") == nil)
    #expect(await store.verify(token: "hd_ffffffffffff.deadbeef") == nil)
    let wrongSecret = "hd_\(creation.client.id).\(String(repeating: "0", count: 64))"
    #expect(await store.verify(token: wrongSecret) == nil)
}

@Test func revokeKillsVerifyAndDeletesSecret() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = InMemorySecretStore()
    let store = GatewayClientStore(directory: dir, secrets: secrets)
    let creation = try await store.create(name: "victim", scopes: .all)

    try await store.revoke(id: creation.client.id)
    #expect(await store.verify(token: creation.token) == nil)
    #expect(await store.list().isEmpty)
    #expect(try secrets.get(account: GatewayClientStore.secretAccount(creation.client.id)) == nil)
}

@Test func clientsPersistAcrossStoreInstances() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let secrets = InMemorySecretStore()
    let first = GatewayClientStore(directory: dir, secrets: secrets)
    let creation = try await first.create(
        name: "durable", scopes: GatewayScopes(models: nil, capabilities: ["chat"]))

    let second = GatewayClientStore(directory: dir, secrets: secrets)
    let listed = await second.list()
    #expect(listed.count == 1)
    #expect(listed[0].id == creation.client.id)
    #expect(listed[0].name == "durable")
    let identity = await second.verify(token: creation.token)
    #expect(identity != nil)
}

@Test func verifyUpdatesLastUsedAt() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = GatewayClientStore(directory: dir, secrets: InMemorySecretStore())
    let creation = try await store.create(name: "used", scopes: .all)
    #expect(creation.client.lastUsedAt == nil)

    _ = await store.verify(token: creation.token)
    let listed = await store.list()
    #expect(listed[0].lastUsedAt != nil)
}
