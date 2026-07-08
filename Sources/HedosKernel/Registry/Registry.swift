import Foundation

public enum RegistryError: Error, Sendable {
    case corruptStore(description: String)
}

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


    public func get(id: String) throws -> ModelRecord? {
        try loadIfNeeded()
        return models[id]
    }

    @discardableResult
    public func setStateIfPresent(id: String, to state: ModelState) throws -> Bool {
        try loadIfNeeded()
        guard models[id] != nil else { return false }
        models[id]?.state = state
        try save()
        return true
    }

    public func list() throws -> [ModelRecord] {
        try loadIfNeeded()
        return models.values.sorted {
            ($0.name.localizedLowercase, $0.id) < ($1.name.localizedLowercase, $1.id)
        }
    }


    private struct Envelope: Codable {
        var schemaVersion: Int
        var models: [ModelRecord]
    }

    private var storeURL: URL {
        directory.appendingPathComponent("models.json")
    }

    private func loadIfNeeded() throws {
        guard !loaded else { return }
        guard FileManager.default.fileExists(atPath: storeURL.path) else {
            loaded = true
            return
        }
        let data = try Data(contentsOf: storeURL)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        do {
            let envelope = try decoder.decode(Envelope.self, from: data)
            models = Dictionary(
                envelope.models.map { ($0.id, $0.droppingVanishedParamValues()) },
                uniquingKeysWith: { _, newer in newer })
            loaded = true
        } catch {
            let quarantineURL = directory.appendingPathComponent(
                "models.json.corrupt-\(Int(Date().timeIntervalSince1970))")
            try? FileManager.default.moveItem(at: storeURL, to: quarantineURL)
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
