import Foundation

public enum PlaceFiles {
    public static let mentionIndexCap = 2000
    public static let menuCapacity = 8

    public static func list(place: String) -> [String] {
        var paths: [String] = []
        let enumerator = FileManager.default.enumerator(
            at: URL(fileURLWithPath: place), includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles])
        while let entry = enumerator?.nextObject() as? URL {
            if paths.count >= mentionIndexCap { break }
            let values = try? entry.resourceValues(
                forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
            guard values?.isRegularFile == true, values?.isSymbolicLink != true else {
                continue
            }
            guard let resolved = try? PlaceBoundary.resolve(entry.path, in: place),
                resolved == entry.path || resolved.hasPrefix(place + "/")
            else { continue }
            guard entry.path.hasPrefix(place + "/") else { continue }
            paths.append(String(entry.path.dropFirst(place.count + 1)))
        }
        return paths.sorted()
    }

    public static func matches(query: String, in paths: [String]) -> [String] {
        paths
            .compactMap { path -> (String, Int)? in
                let filename = (path as NSString).lastPathComponent
                if let nameScore = PromptComposer.matchScore(query, against: filename) {
                    return (path, nameScore)
                }
                if let pathScore = PromptComposer.matchScore(query, against: path) {
                    return (path, pathScore + 3)
                }
                return nil
            }
            .sorted {
                ($0.1, $0.0.count, $0.0) < ($1.1, $1.0.count, $1.0)
            }
            .prefix(menuCapacity)
            .map(\.0)
    }
}
