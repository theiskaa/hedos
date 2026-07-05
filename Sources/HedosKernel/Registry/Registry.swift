import Foundation

public enum RegistryError: Error, Sendable {
    /// The store exists but cannot be decoded. Never silently discarded —
    /// surfaced so the user (or a future migration) can decide.
    case corruptStore(description: String)
}

/// The persisted shelf: all registered models, backed by `models.json` in
/// the given directory. Storage location is injected so tests run against
/// temp dirs; the app passes `Registry.defaultDirectory()`.
public actor Registry {
    public let directory: URL

    private var models: [String: ModelRecord] = [:]
    private var loaded = false

    public init(directory: URL) {
        self.directory = directory
    }

    public static func defaultDirectory() -> URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Hedos", isDirectory: true)
    }

    // MARK: - Mutations (persist immediately)

    /// Upsert keyed by the record's stable ID: re-registering the same
    /// source updates the record instead of duplicating it.
    public func register(_ record: ModelRecord) throws {
        try loadIfNeeded()
        models[record.id] = record
        try save()
    }

    @discardableResult
    public func unregister(id: String) throws -> ModelRecord? {
        try loadIfNeeded()
        let removed = models.removeValue(forKey: id)
        if removed != nil { try save() }
        return removed
    }

    // MARK: - Queries

    public func get(id: String) throws -> ModelRecord? {
        try loadIfNeeded()
        return models[id]
    }

    public func list() throws -> [ModelRecord] {
        try loadIfNeeded()
        return models.values.sorted {
            ($0.name.localizedLowercase, $0.id) < ($1.name.localizedLowercase, $1.id)
        }
    }

    // MARK: - Persistence

    private struct Envelope: Codable {
        var schemaVersion: Int
        var models: [ModelRecord]
    }

    private var storeURL: URL {
        directory.appendingPathComponent("models.json")
    }

    private func loadIfNeeded() throws {
        guard !loaded else { return }
        loaded = true
        guard FileManager.default.fileExists(atPath: storeURL.path) else { return }
        let data = try Data(contentsOf: storeURL)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        do {
            let envelope = try decoder.decode(Envelope.self, from: data)
            models = Dictionary(
                envelope.models.map { ($0.id, $0) },
                uniquingKeysWith: { _, newer in newer })
        } catch {
            throw RegistryError.corruptStore(description: String(describing: error))
        }
    }

    private func save() throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        let envelope = Envelope(
            schemaVersion: 1,
            models: models.values.sorted { $0.id < $1.id })
        let data = try encoder.encode(envelope)
        try data.write(to: storeURL, options: .atomic)
    }
}
