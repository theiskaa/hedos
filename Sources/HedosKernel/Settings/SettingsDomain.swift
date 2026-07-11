import Foundation

public protocol SettingsDomain: Codable, Sendable, Equatable {
    static var domainName: String { get }
    static func compatibilityRead(from directory: URL) -> Self?
    init()
}

extension SettingsDomain {
    public static func compatibilityRead(from directory: URL) -> Self? {
        nil
    }
}

extension KeyedDecodingContainer {
    func lenient<T: Decodable>(_ type: T.Type, _ key: Key, fallback: T) -> T {
        (try? decodeIfPresent(type, forKey: key)) ?? fallback
    }

    func lenient<T: Decodable>(_ type: T.Type, _ key: Key) -> T? {
        try? decodeIfPresent(type, forKey: key)
    }
}
