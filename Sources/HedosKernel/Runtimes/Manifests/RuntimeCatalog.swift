import Foundation

public struct RuntimeCatalog: Sendable {
    public let directory: URL
    let reservedIDs: Set<String>

    init(directory: URL, reservedIDs: Set<String>) {
        self.directory = directory
        self.reservedIDs = reservedIDs
    }

    public func ensuredDirectory() -> URL {
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        return directory
    }

    func load() -> (manifests: [RuntimeManifest], issues: [String]) {
        UserRuntimeStore(directory: directory).load(reservedIDs: reservedIDs)
    }

    public func installedCommunity() -> [RuntimeManifest] {
        load().manifests.filter { $0.provenance?.isCommunity == true }
    }
}
