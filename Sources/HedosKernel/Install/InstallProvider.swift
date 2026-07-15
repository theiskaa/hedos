import Foundation

public struct InstallProviderID: RawRepresentable, Codable, Hashable, Sendable,
    ExpressibleByStringLiteral, CustomStringConvertible
{
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }
    public var description: String { rawValue }

    public static let ollama = InstallProviderID(rawValue: "ollama")
    public static let huggingface = InstallProviderID(rawValue: "huggingface")
}

public enum InstallAvailability: Sendable, Hashable {
    case ready
    case unavailable(hint: String)
}

public struct InstallProviderStatus: Sendable, Hashable, Identifiable {
    public let id: InstallProviderID
    public let displayName: String
    public let sourceKind: SourceKind
    public let supportsSearch: Bool
    public let availability: InstallAvailability

    public init(
        id: InstallProviderID, displayName: String, sourceKind: SourceKind,
        supportsSearch: Bool, availability: InstallAvailability
    ) {
        self.id = id
        self.displayName = displayName
        self.sourceKind = sourceKind
        self.supportsSearch = supportsSearch
        self.availability = availability
    }
}

public protocol InstallProvider: Sendable {
    var id: InstallProviderID { get }
    var displayName: String { get }
    var sourceKind: SourceKind { get }
    var supportsSearch: Bool { get }
    func availability() async -> InstallAvailability
    func search(matching query: String, limit: Int) async throws -> [InstallSearchHit]
    func plan(reference: String) async throws -> InstallPlan
    func install(_ plan: InstallPlan) -> AsyncThrowingStream<InstallStreamEvent, Error>
}
