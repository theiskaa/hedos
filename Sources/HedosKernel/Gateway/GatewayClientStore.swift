import CryptoKit
import Foundation

public struct GatewayClient: Codable, Sendable, Hashable, Identifiable {
    public let id: String
    public var name: String
    public var scopes: GatewayScopes
    public var createdAt: Date
    public var lastUsedAt: Date?

    public init(
        id: String, name: String, scopes: GatewayScopes, createdAt: Date,
        lastUsedAt: Date? = nil
    ) {
        self.id = id
        self.name = name
        self.scopes = scopes
        self.createdAt = createdAt
        self.lastUsedAt = lastUsedAt
    }
}

public struct GatewayClientCreation: Sendable {
    public let client: GatewayClient
    public let token: String
}

public actor GatewayClientStore {
    private let fileURL: URL
    private let secrets: any SecretStore
    private var clients: [GatewayClient] = []
    private var loaded = false
    private var persistedUse: [String: Date] = [:]

    public init(directory: URL, secrets: any SecretStore) {
        self.fileURL = directory.appendingPathComponent("clients.json")
        self.secrets = secrets
    }

    static func secretAccount(_ clientID: String) -> String {
        "gateway:\(clientID)"
    }

    static func hash(_ secret: String) -> String {
        SHA256.hash(data: Data(secret.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    static func randomHex(bytes count: Int) -> String {
        var generator = SystemRandomNumberGenerator()
        return (0..<count).map { _ in
            String(format: "%02x", UInt8.random(in: .min ... .max, using: &generator))
        }.joined()
    }

    static func constantTimeEquals(_ a: String, _ b: String) -> Bool {
        let left = Array(a.utf8)
        let right = Array(b.utf8)
        guard left.count == right.count else { return false }
        var difference: UInt8 = 0
        for index in left.indices {
            difference |= left[index] ^ right[index]
        }
        return difference == 0
    }

    public func create(name: String, scopes: GatewayScopes) throws -> GatewayClientCreation {
        loadIfNeeded()
        let clientID = Self.randomHex(bytes: 6)
        let secret = Self.randomHex(bytes: 32)
        try secrets.set(Self.hash(secret), account: Self.secretAccount(clientID))
        let client = GatewayClient(
            id: clientID, name: name, scopes: scopes, createdAt: Date())
        clients.append(client)
        try persist()
        return GatewayClientCreation(client: client, token: "hd_\(clientID).\(secret)")
    }

    public func list() -> [GatewayClient] {
        loadIfNeeded()
        return clients
    }

    public func revoke(id: String) throws {
        loadIfNeeded()
        clients.removeAll { $0.id == id }
        try secrets.delete(account: Self.secretAccount(id))
        try persist()
    }

    public func verify(token: String) -> GatewayIdentity? {
        loadIfNeeded()
        guard token.hasPrefix("hd_") else { return nil }
        let remainder = token.dropFirst(3)
        let parts = remainder.split(separator: ".", maxSplits: 1)
        guard parts.count == 2 else { return nil }
        let clientID = String(parts[0])
        let secret = String(parts[1])
        guard let index = clients.firstIndex(where: { $0.id == clientID }) else { return nil }
        guard let storedHash = try? secrets.get(account: Self.secretAccount(clientID))
        else { return nil }
        guard Self.constantTimeEquals(Self.hash(secret), storedHash) else { return nil }
        touch(at: index)
        let client = clients[index]
        return GatewayIdentity(clientID: client.id, name: client.name, scopes: client.scopes)
    }

    private func touch(at index: Int) {
        let now = Date()
        clients[index].lastUsedAt = now
        let persisted = persistedUse[clients[index].id]
        if persisted == nil || now.timeIntervalSince(persisted!) > 60 {
            persistedUse[clients[index].id] = now
            try? persist()
        }
    }

    private func loadIfNeeded() {
        guard !loaded else { return }
        guard let data = try? Data(contentsOf: fileURL) else {
            loaded = true
            return
        }
        guard let decoded = try? StoreCoding.decoder().decode([GatewayClient].self, from: data)
        else {
            StoreCoding.quarantine(fileURL)
            loaded = true
            return
        }
        clients = decoded
        for client in clients {
            if let used = client.lastUsedAt { persistedUse[client.id] = used }
        }
        loaded = true
    }

    private func persist() throws {
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try StoreCoding.encoder().encode(clients).write(to: fileURL, options: .atomic)
    }
}
