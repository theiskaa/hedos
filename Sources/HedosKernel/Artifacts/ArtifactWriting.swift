import Foundation

public protocol ArtifactWriting: Sendable {
    func write(_ data: Data, fileExtension: String, for job: Job) async throws -> Artifact
}

public struct ProvenanceArtifactWriter: ArtifactWriting {
    private let store: ArtifactStore
    private let registry: Registry

    public init(store: ArtifactStore, registry: Registry) {
        self.store = store
        self.registry = registry
    }

    public func write(
        _ data: Data, fileExtension: String, for job: Job
    ) async throws -> Artifact {
        let record = try await registry.get(id: job.modelID)
        let startedAt = job.startedAt ?? job.submittedAt
        let durationMs = Int((Date().timeIntervalSince(startedAt) * 1000).rounded())
        return try await store.store(
            ArtifactDraft(
                data: data,
                fileExtension: fileExtension,
                preview: job.preview,
                model: record?.name ?? job.modelID,
                modelID: job.modelID,
                runtime: record?.runtime.id?.rawValue ?? "unresolved",
                capability: job.capability,
                params: job.payload,
                jobID: job.id,
                durationMs: max(durationMs, 0)))
    }
}
