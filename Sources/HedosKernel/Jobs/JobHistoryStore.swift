import Foundation

public actor JobHistoryStore {
    public let directory: URL
    public let limit: Int

    private var jobs: [Job] = []
    private var loaded = false

    public init(directory: URL, limit: Int = 50) {
        self.directory = directory
        self.limit = limit
    }

    public func record(_ job: Job) throws {
        loadIfNeeded()
        jobs.removeAll { $0.id == job.id }
        jobs.append(job)
        jobs.sort { ($0.submittedAt, $0.id) > ($1.submittedAt, $1.id) }
        if jobs.count > limit {
            jobs.removeLast(jobs.count - limit)
        }
        try save()
    }

    public func list() throws -> [Job] {
        loadIfNeeded()
        return jobs
    }

    public func get(id: String) throws -> Job? {
        loadIfNeeded()
        return jobs.first { $0.id == id }
    }

    private struct Envelope: Codable {
        var schemaVersion: Int
        var jobs: [Job]
    }

    private var storeURL: URL {
        directory.appendingPathComponent("jobs.json")
    }

    private func loadIfNeeded() {
        guard !loaded else { return }
        loaded = true
        guard FileManager.default.fileExists(atPath: storeURL.path) else { return }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        guard let data = try? Data(contentsOf: storeURL),
            let envelope = try? decoder.decode(Envelope.self, from: data)
        else { return }
        jobs = envelope.jobs
    }

    private func save() throws {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        let envelope = Envelope(schemaVersion: 1, jobs: jobs)
        let data = try encoder.encode(envelope)
        try data.write(to: storeURL, options: .atomic)
    }
}
