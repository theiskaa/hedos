import Foundation

enum CanonicalPath {
    static func of(_ path: String) -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard let real = realpath(path, &buffer) else {
            return URL(fileURLWithPath: path).resolvingSymlinksInPath().path
        }
        return String(cString: real)
    }

    static func of(_ url: URL) -> String {
        of(url.path)
    }
}
