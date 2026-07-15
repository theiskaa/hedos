import Foundation

public struct InstallPlanFile: Sendable, Hashable {
    public let path: String
    public let bytes: Int64?

    public init(path: String, bytes: Int64? = nil) {
        self.path = path
        self.bytes = bytes
    }
}

public struct InstallPlan: Sendable, Hashable {
    public let provider: InstallProviderID
    public let reference: String
    public let displayName: String
    public let revision: String?
    public let files: [InstallPlanFile]
    public let totalBytes: Int64?
    public let remainingBytes: Int64?
    public let destination: String
    public let requiresAuth: Bool

    public init(
        provider: InstallProviderID, reference: String, displayName: String,
        revision: String? = nil, files: [InstallPlanFile] = [], totalBytes: Int64? = nil,
        remainingBytes: Int64? = nil, destination: String, requiresAuth: Bool = false
    ) {
        self.provider = provider
        self.reference = reference
        self.displayName = displayName
        self.revision = revision
        self.files = files
        self.totalBytes = totalBytes
        self.remainingBytes = remainingBytes
        self.destination = destination
        self.requiresAuth = requiresAuth
    }
}

public struct InstallSearchHit: Sendable, Hashable, Identifiable {
    public let provider: InstallProviderID
    public let reference: String
    public let name: String
    public let downloads: Int?
    public let likes: Int?
    public let updatedAt: Date?

    public init(
        provider: InstallProviderID, reference: String, name: String,
        downloads: Int? = nil, likes: Int? = nil, updatedAt: Date? = nil
    ) {
        self.provider = provider
        self.reference = reference
        self.name = name
        self.downloads = downloads
        self.likes = likes
        self.updatedAt = updatedAt
    }

    public var id: String { "\(provider.rawValue)|\(reference)" }
}

public struct ActiveInstall: Sendable, Hashable, Identifiable {
    public let id: String
    public let provider: InstallProviderID
    public let reference: String
    public let displayName: String
    public let totalBytes: Int64?
    public var progress: InstallProgress
    public let startedAt: Date

    public init(
        id: String, provider: InstallProviderID, reference: String, displayName: String,
        totalBytes: Int64? = nil, progress: InstallProgress = InstallProgress(),
        startedAt: Date = Date()
    ) {
        self.id = id
        self.provider = provider
        self.reference = reference
        self.displayName = displayName
        self.totalBytes = totalBytes
        self.progress = progress
        self.startedAt = startedAt
    }
}
