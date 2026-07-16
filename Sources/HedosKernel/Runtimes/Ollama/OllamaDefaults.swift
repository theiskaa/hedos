import Foundation

enum OllamaDefaults {
    static func modelsRoot(environment: [String: String], home: URL) -> URL {
        if let custom = environment["OLLAMA_MODELS"], !custom.isEmpty {
            return URL(
                fileURLWithPath: (custom as NSString).expandingTildeInPath, isDirectory: true)
        }
        return home.appendingPathComponent(".ollama/models")
    }

    static func displayModelsPath(environment: [String: String], home: URL) -> String {
        let path = modelsRoot(environment: environment, home: home).standardizedFileURL.path
        let homePath = home.standardizedFileURL.path
        guard path == homePath || path.hasPrefix(homePath + "/") else { return path }
        return "~" + path.dropFirst(homePath.count)
    }
}
