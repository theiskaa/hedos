import Foundation

public struct DiscoverySummary: Sendable {
    public struct KindStat: Sendable, Hashable {
        public var count: Int
        public var bytes: Int64
    }

    public var perKind: [SourceKind: KindStat]
    public var totalCount: Int
    public var totalBytes: Int64
    public var duplicates: [DuplicateGroup]
    public var issues: [String]
    public var failedKinds: Set<SourceKind> = []

    public var headline: String {
        guard totalCount > 0 else {
            return "No models found on this Mac yet."
        }
        var parts: [String] = []
        let ordered: [(SourceKind, String)] = [
            (.ollama, "in Ollama"),
            (.huggingfaceCache, "in the Hugging Face cache"),
            (.lmStudio, "in LM Studio"),
            (.builtin, "built in"),
        ]
        for (kind, label) in ordered {
            if let stat = perKind[kind], stat.count > 0 {
                parts.append("\(stat.count) \(label)")
            }
        }
        let looseCount = (perKind[.file]?.count ?? 0) + (perKind[.folder]?.count ?? 0)
        if looseCount > 0 {
            parts.append(looseCount == 1 ? "1 loose file" : "\(looseCount) loose files")
        }
        let models = totalCount == 1 ? "1 model" : "\(totalCount) models"
        let breakdown = parts.isEmpty ? "" : " — \(parts.joined(separator: ", "))"
        return "Found \(models) on this Mac\(breakdown). Total: \(Self.formatBytes(totalBytes))."
    }

    public static func formatBytes(_ bytes: Int64) -> String {
        switch bytes {
        case (1 << 30)...:
            let value = Double(bytes) / Double(1 << 30)
            let formatted = String(format: "%.1f", value)
            return "\(formatted.hasSuffix(".0") ? String(formatted.dropLast(2)) : formatted) GB"
        case (1 << 20)...:
            return "\(bytes >> 20) MB"
        case 1024...:
            return "\(bytes >> 10) KB"
        default:
            return "\(bytes) B"
        }
    }
}

public actor DiscoveryService {
    private let scanners: [any StoreScanner]
    private let duplicateThreshold: Int64

    public init(
        scanners: [any StoreScanner],
        duplicateThreshold: Int64 = DuplicateDetector.defaultThreshold
    ) {
        self.scanners = scanners
        self.duplicateThreshold = duplicateThreshold
    }

    public func discover(into registry: Registry) async throws -> DiscoverySummary {
        var discovered: [DiscoveredModel] = []
        var issues: [String] = []
        var failedKinds: Set<SourceKind> = []
        await withTaskGroup(of: ScanResult.self) { group in
            for scanner in scanners {
                group.addTask { await scanner.scan() }
            }
            for await result in group {
                discovered.append(contentsOf: result.discovered)
                issues.append(contentsOf: result.issues)
                failedKinds.formUnion(result.failedKinds)
            }
        }
        for kind in failedKinds.map(\.rawValue).sorted() {
            issues.append("skipped the missing check for \(kind) — its store could not be read")
        }

        let existing = try await registry.list()
        let existingByID = Dictionary(
            existing.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        var seenIDs = Set<String>()
        var toRegister: [ModelRecord] = []
        var newRecordIDs = Set<String>()

        for model in discovered {
            let id = ModelRecord.stableID(for: model.source)
            guard seenIDs.insert(id).inserted else { continue }

            if var record = existingByID[id] {
                record.contentFingerprint = fingerprint(for: model, existing: record)
                record.name = model.name
                record.source = model.source
                record.footprintMB = Int(model.footprintBytes / (1 << 20))
                record.primaryWeightPath = model.primaryWeightPath
                if let modality = model.modalityHint { record.modality = modality }
                if !model.capabilitiesHint.isEmpty { record.capabilities = model.capabilitiesHint }
                if let contextLength = model.contextLengthHint {
                    record.contextLength = contextLength
                }
                if let hasChatTemplate = model.hasChatTemplateHint {
                    record.hasChatTemplate = hasChatTemplate
                }
                if let stopTokens = model.stopTokensHint {
                    record.stopTokens = stopTokens
                }
                record.execution = model.executionHint
                record.downloading = model.downloading
                if record.state == .missing { record.state = .unresolved }
                toRegister.append(record)
            } else {
                var record = ModelRecord(
                    name: model.name,
                    modality: model.modalityHint ?? .unknown,
                    capabilities: model.capabilitiesHint,
                    source: model.source,
                    execution: model.executionHint,
                    footprintMB: Int(model.footprintBytes / (1 << 20)),
                    state: .unresolved,
                    contextLength: model.contextLengthHint,
                    hasChatTemplate: model.hasChatTemplateHint,
                    stopTokens: model.stopTokensHint)
                record.primaryWeightPath = model.primaryWeightPath
                record.downloading = model.downloading
                record.contentFingerprint = fingerprint(for: model, existing: nil)
                toRegister.append(record)
                newRecordIDs.insert(id)
            }
        }

        let scannedKinds = Set(scanners.flatMap(\.kinds)).subtracting(failedKinds)
        for record in existing
        where scannedKinds.contains(record.source.kind) && !seenIDs.contains(record.id)
            && record.state != .missing && !Self.weightsPresent(record)
        {
            var stale = record
            stale.state = .missing
            toRegister.append(stale)
        }

        let migratedAwayIDs = migrateMovedConfig(
            into: &toRegister, newRecordIDs: newRecordIDs,
            missingCandidates: existing.filter {
                !seenIDs.contains($0.id)
                    && ($0.state == .missing || scannedKinds.contains($0.source.kind))
            })
        try await registry.register(
            contentsOf: toRegister.filter { !migratedAwayIDs.contains($0.id) })
        for id in migratedAwayIDs {
            try await registry.unregister(id: id)
        }

        var perKind: [SourceKind: DiscoverySummary.KindStat] = [:]
        for model in discovered {
            var stat = perKind[model.source.kind] ?? .init(count: 0, bytes: 0)
            stat.count += 1
            stat.bytes += model.footprintBytes
            perKind[model.source.kind] = stat
        }
        return DiscoverySummary(
            perKind: perKind,
            totalCount: discovered.count,
            totalBytes: discovered.reduce(0) { $0 + $1.footprintBytes },
            duplicates: DuplicateDetector.detect(in: discovered, threshold: duplicateThreshold),
            issues: issues,
            failedKinds: failedKinds)
    }

    private static func weightsPresent(_ record: ModelRecord) -> Bool {
        guard let path = record.primaryWeightPath, !path.isEmpty else { return false }
        if FileManager.default.fileExists(atPath: path) { return true }
        return FileManager.default.fileExists(atPath: record.source.path)
    }

    private func fingerprint(for model: DiscoveredModel, existing: ModelRecord?) -> String? {
        guard let path = model.primaryWeightPath else { return existing?.contentFingerprint }
        if let existing, let known = existing.contentFingerprint,
            existing.primaryWeightPath == model.primaryWeightPath,
            existing.footprintMB == Int(model.footprintBytes / (1 << 20))
        {
            return known
        }
        return DuplicateDetector.fingerprint(ofPath: path)
    }

    private func migrateMovedConfig(
        into toRegister: inout [ModelRecord], newRecordIDs: Set<String>,
        missingCandidates: [ModelRecord]
    ) -> Set<String> {
        var claimed = Set<String>()
        for index in toRegister.indices where newRecordIDs.contains(toRegister[index].id) {
            guard let fingerprint = toRegister[index].contentFingerprint else { continue }
            let matches = missingCandidates.filter {
                $0.contentFingerprint == fingerprint
                    && $0.footprintMB == toRegister[index].footprintMB
                    && !claimed.contains($0.id)
            }
            guard matches.count == 1, let orphan = matches.first else { continue }
            toRegister[index].paramValues = orphan.paramValues
            toRegister[index].systemPrompt = orphan.systemPrompt
            toRegister[index].alias = orphan.alias
            claimed.insert(orphan.id)
        }
        return claimed
    }
}
