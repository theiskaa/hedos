import Foundation

public enum PlaceFiles {
    public static let mentionIndexCap = 2000
    public static let menuCapacity = 20

    public static func list(place: String) -> [String] {
        var paths: [String] = []
        let ignore = PlaceIgnore.load(place: place)
        let enumerator = FileManager.default.enumerator(
            at: URL(fileURLWithPath: place),
            includingPropertiesForKeys: [.isRegularFileKey, .isDirectoryKey],
            options: [.skipsHiddenFiles])
        while let entry = enumerator?.nextObject() as? URL {
            if paths.count >= mentionIndexCap { break }
            let values = try? entry.resourceValues(
                forKeys: [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey])
            guard entry.path.hasPrefix(place + "/") else { continue }
            let relative = String(entry.path.dropFirst(place.count + 1))
            if values?.isDirectory == true {
                if ignore.ignored(relative, isDirectory: true) {
                    enumerator?.skipDescendants()
                }
                continue
            }
            guard values?.isRegularFile == true, values?.isSymbolicLink != true else {
                continue
            }
            guard !ignore.ignored(relative, isDirectory: false) else { continue }
            guard let resolved = try? PlaceBoundary.resolve(entry.path, in: place),
                resolved == entry.path || resolved.hasPrefix(place + "/")
            else { continue }
            paths.append(relative)
        }
        return paths.sorted()
    }

    public static func matches(query: String, in paths: [String]) -> [String] {
        guard !query.isEmpty else {
            return Array(paths.sorted().prefix(menuCapacity))
        }
        return paths
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
