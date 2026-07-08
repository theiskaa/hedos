import Foundation
import Security

public protocol SecretStore: Sendable {
    func set(_ secret: String, account: String) throws
    func get(account: String) throws -> String?
    func delete(account: String) throws
}

public struct KeychainStore: SecretStore {
    static let service = "com.hedos.endpoint"

    public init() {}

    public func set(_ secret: String, account: String) throws {
        try delete(account: account)
        let attributes: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: Self.service,
            kSecAttrAccount: account,
            kSecValueData: Data(secret.utf8),
        ]
        let status = SecItemAdd(attributes as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw KernelError.runtimeFailed("keychain write failed with status \(status)")
        }
    }

    public func get(account: String) throws -> String? {
        if let secret = try rawGet(account: account) {
            return secret
        }
        guard let legacyAccount = Self.legacyAccount(for: account),
            let secret = try rawGet(account: legacyAccount)
        else {
            return nil
        }
        try? set(secret, account: account)
        try? delete(account: legacyAccount)
        return secret
    }

    private func rawGet(account: String) throws -> String? {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: Self.service,
            kSecAttrAccount: account,
            kSecReturnData: true,
            kSecMatchLimit: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else {
            throw KernelError.runtimeFailed("keychain read failed with status \(status)")
        }
        return String(data: data, encoding: .utf8)
    }

    static func legacyAccount(for account: String) -> String? {
        guard let url = URL(string: account), url.scheme == "http", let host = url.host else {
            return nil
        }
        let port = url.port.map { ":\($0)" } ?? ""
        let legacy = "\(host)\(port)"
        return legacy != account ? legacy : nil
    }

    public func delete(account: String) throws {
        let query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: Self.service,
            kSecAttrAccount: account,
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KernelError.runtimeFailed("keychain delete failed with status \(status)")
        }
    }
}

public final class InMemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [String: String] = [:]

    public init() {}

    public func set(_ secret: String, account: String) throws {
        lock.lock()
        storage[account] = secret
        lock.unlock()
    }

    public func get(account: String) throws -> String? {
        lock.lock()
        defer { lock.unlock() }
        return storage[account]
    }

    public func delete(account: String) throws {
        lock.lock()
        storage[account] = nil
        lock.unlock()
    }
}
