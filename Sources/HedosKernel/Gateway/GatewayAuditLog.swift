import Foundation

public struct GatewayAuditEntry: Codable, Sendable, Hashable {
    public var ts: Date
    public var client: String?
    public var clientName: String?
    public var method: String
    public var route: String
    public var model: String?
    public var capability: String?
    public var outcome: String
    public var status: Int
    public var durationMs: Int
    public var detail: String?

    public init(
        ts: Date = Date(), client: String? = nil, clientName: String? = nil, method: String,
        route: String, model: String? = nil, capability: String? = nil, outcome: String,
        status: Int, durationMs: Int, detail: String? = nil
    ) {
        self.ts = ts
        self.client = client
        self.clientName = clientName
        self.method = method
        self.route = route
        self.model = model
        self.capability = capability
        self.outcome = outcome
        self.status = status
        self.durationMs = durationMs
        self.detail = detail
    }
}

public actor GatewayAuditLog {
    private let fileURL: URL
    private let maxBytes: Int
    private let generations = 3
    private let unauthorizedWindowSeconds: TimeInterval = 60
    private var unauthorizedWindowStart: Date?
    private var unauthorizedCount = 0

    public init(directory: URL, maxBytes: Int = 5_242_880) {
        self.fileURL = directory.appendingPathComponent("audit.jsonl")
        self.maxBytes = maxBytes
    }

    public func appendUnauthorized(_ entry: GatewayAuditEntry) {
        if let start = unauthorizedWindowStart,
            entry.ts.timeIntervalSince(start) < unauthorizedWindowSeconds
        {
            unauthorizedCount += 1
            return
        }
        flushUnauthorized()
        unauthorizedWindowStart = entry.ts
        unauthorizedCount = 0
        writeEntry(entry)
    }

    private func flushUnauthorized() {
        defer {
            unauthorizedWindowStart = nil
            unauthorizedCount = 0
        }
        guard let start = unauthorizedWindowStart, unauthorizedCount > 0 else { return }
        writeEntry(
            GatewayAuditEntry(
                ts: start, method: "-", route: "-", outcome: "unauthorized", status: 401,
                durationMs: 0,
                detail: "\(unauthorizedCount) more unauthenticated requests rejected"))
    }

    public func append(_ entry: GatewayAuditEntry) {
        flushUnauthorized()
        writeEntry(entry)
    }

    private func writeEntry(_ entry: GatewayAuditEntry) {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        guard var line = try? encoder.encode(entry) else { return }
        line.append(contentsOf: [0x0A])
        let manager = FileManager.default
        try? manager.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        rotateIfNeeded()
        if let handle = try? FileHandle(forWritingTo: fileURL) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: line)
        } else {
            try? line.write(to: fileURL)
        }
    }

    public func tail(limit: Int) -> [GatewayAuditEntry] {
        guard let data = try? Data(contentsOf: fileURL),
            let text = String(data: data, encoding: .utf8)
        else { return [] }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let lines = text.split(separator: "\n").suffix(limit)
        return lines.compactMap { try? decoder.decode(GatewayAuditEntry.self, from: Data($0.utf8)) }
    }

    public nonisolated var logURL: URL { fileURL }

    private func rotateIfNeeded() {
        let manager = FileManager.default
        guard let attributes = try? manager.attributesOfItem(atPath: fileURL.path),
            let size = attributes[.size] as? Int, size > maxBytes
        else { return }
        let oldest = rotatedURL(generation: generations - 1)
        try? manager.removeItem(at: oldest)
        for generation in stride(from: generations - 2, through: 1, by: -1) {
            let source = rotatedURL(generation: generation)
            if manager.fileExists(atPath: source.path) {
                try? manager.moveItem(at: source, to: rotatedURL(generation: generation + 1))
            }
        }
        try? manager.moveItem(at: fileURL, to: rotatedURL(generation: 1))
    }

    private func rotatedURL(generation: Int) -> URL {
        fileURL.deletingLastPathComponent().appendingPathComponent("audit.\(generation).jsonl")
    }
}
