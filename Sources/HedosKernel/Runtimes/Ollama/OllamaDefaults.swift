import Foundation

enum OllamaDefaults {
    static let baseURL = URL(string: "http://127.0.0.1:11434")!
    static let notInstalledHint = "Ollama isn't installed. Get it from ollama.com."
    static let probeTimeout: TimeInterval = 2

    static func daemonReachable(baseURL: URL, session: URLSession) async -> Bool {
        var request = URLRequest(url: baseURL.appendingPathComponent("api/tags"))
        request.timeoutInterval = probeTimeout
        guard let (_, response) = try? await session.data(for: request) else { return false }
        return (response as? HTTPURLResponse)?.statusCode == 200
    }

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
