import Foundation

public enum HarnessError: Error, Sendable, Equatable, LocalizedError {
    case outsidePlace(requested: String)
    case invalidPath(requested: String)

    public var errorDescription: String? {
        switch self {
        case .outsidePlace(let requested):
            "\(requested) is outside this conversation's folder."
        case .invalidPath(let requested):
            "\(requested) is not a usable path."
        }
    }
}

public enum PlaceBoundary {
    public static func canonical(_ path: String) -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard let real = realpath(path, &buffer) else {
            return URL(fileURLWithPath: path).resolvingSymlinksInPath().path
        }
        return String(cString: real)
    }

    public static func resolve(_ requested: String, in place: String) throws -> String {
        let trimmed = requested.trimmingCharacters(in: .whitespacesAndNewlines)
        let joined: String
        if trimmed.isEmpty || trimmed == "." {
            joined = place
        } else if trimmed.hasPrefix("/") {
            joined = trimmed
        } else if trimmed.hasPrefix("~") {
            throw HarnessError.outsidePlace(requested: requested)
        } else {
            joined = place + "/" + trimmed
        }

        let resolved = try canonicalizeAllowingMissingTail(joined, requested: requested)
        guard resolved == place || resolved.hasPrefix(place + "/") else {
            throw HarnessError.outsidePlace(requested: requested)
        }
        return resolved
    }

    private static func canonicalizeAllowingMissingTail(
        _ path: String, requested: String
    ) throws -> String {
        if FileManager.default.fileExists(atPath: path) {
            return canonical(path)
        }
        var components = URL(fileURLWithPath: path).pathComponents
        var tail: [String] = []
        while components.count > 1 {
            let candidate = NSString.path(withComponents: components)
            if FileManager.default.fileExists(atPath: candidate) {
                for component in tail {
                    guard component != "." && component != ".." else {
                        throw HarnessError.invalidPath(requested: requested)
                    }
                }
                let base = canonical(candidate)
                return tail.isEmpty ? base : base + "/" + tail.joined(separator: "/")
            }
            tail.insert(components.removeLast(), at: 0)
        }
        throw HarnessError.invalidPath(requested: requested)
    }
}
