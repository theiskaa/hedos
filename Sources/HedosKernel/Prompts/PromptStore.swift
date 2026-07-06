import Foundation

public actor PromptStore {
    public let directory: URL

    private var prompts: [String: Prompt] = [:]
    private var loaded = false

    public init(directory: URL) {
        self.directory = directory
    }

    public func list() -> [Prompt] {
        loadIfNeeded()
        return prompts.values.sorted {
            ($0.title.localizedLowercase, $0.id) < ($1.title.localizedLowercase, $1.id)
        }
    }

    public func get(id: String) -> Prompt? {
        loadIfNeeded()
        return prompts[id]
    }

    @discardableResult
    public func save(_ prompt: Prompt) throws -> Prompt {
        loadIfNeeded()
        var updated = prompt
        updated.updatedAt = Date()
        prompts[updated.id] = updated
        try write(updated)
        return updated
    }

    public func delete(id: String) {
        loadIfNeeded()
        guard prompts.removeValue(forKey: id) != nil else { return }
        try? FileManager.default.removeItem(at: fileURL(id))
    }

    private func fileURL(_ id: String) -> URL {
        directory.appendingPathComponent("\(id).json")
    }

    private func loadIfNeeded() {
        guard !loaded else { return }
        loaded = true
        let files =
            (try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]))
            ?? []
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        for file in files where file.pathExtension.lowercased() == "json" {
            guard let data = try? Data(contentsOf: file),
                let decoded = try? decoder.decode(Prompt.self, from: data)
            else { continue }
            let prompt =
                decoded.id.isEmpty
                ? decoded.identified(as: file.deletingPathExtension().lastPathComponent)
                : decoded
            prompts[prompt.id] = prompt
        }
    }

    private func write(_ prompt: Prompt) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        try encoder.encode(prompt).write(to: fileURL(prompt.id), options: .atomic)
    }
}
