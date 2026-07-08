import Foundation

public actor PipelineStore {
    public let directory: URL

    private var pipelines: [String: Pipeline] = [:]
    private var loaded = false

    public init(directory: URL) {
        self.directory = directory
    }

    public func list() -> [Pipeline] {
        loadIfNeeded()
        return pipelines.values.sorted {
            ($0.name.localizedLowercase, $0.id) < ($1.name.localizedLowercase, $1.id)
        }
    }

    public func get(id: String) -> Pipeline? {
        loadIfNeeded()
        return pipelines[id]
    }

    @discardableResult
    public func save(_ pipeline: Pipeline) throws -> Pipeline {
        loadIfNeeded()
        var updated = pipeline
        updated.updatedAt = Date()
        pipelines[updated.id] = updated
        try write(updated)
        return updated
    }

    public func delete(id: String) {
        loadIfNeeded()
        guard pipelines.removeValue(forKey: id) != nil else { return }
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
                let decoded = try? decoder.decode(Pipeline.self, from: data)
            else { continue }
            pipelines[decoded.id] = decoded
        }
    }

    private func write(_ pipeline: Pipeline) throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        try encoder.encode(pipeline).write(to: fileURL(pipeline.id), options: .atomic)
    }
}
