import Foundation

public enum GallerySort: String, CaseIterable, Hashable, Sendable {
    case newestFirst
    case oldestFirst
}

public struct GalleryModel: Hashable, Sendable, Identifiable {
    public let id: String
    public let name: String

    public init(id: String, name: String) {
        self.id = id
        self.name = name
    }
}

public enum Gallery {
    public static func models(in artifacts: [Artifact]) -> [GalleryModel] {
        var seen = Set<String>()
        return artifacts.sorted(by: newest).compactMap { artifact in
            guard seen.insert(artifact.modelID).inserted else { return nil }
            return GalleryModel(id: artifact.modelID, name: artifact.model)
        }
    }

    public static func arrange(
        _ artifacts: [Artifact], modelID: String? = nil, sort: GallerySort = .newestFirst
    ) -> [Artifact] {
        let filtered = modelID.map { id in artifacts.filter { $0.modelID == id } } ?? artifacts
        switch sort {
        case .newestFirst:
            return filtered.sorted(by: newest)
        case .oldestFirst:
            return filtered.sorted { newest($1, $0) }
        }
    }

    private static func newest(_ lhs: Artifact, _ rhs: Artifact) -> Bool {
        (lhs.createdAt, lhs.id) > (rhs.createdAt, rhs.id)
    }
}
