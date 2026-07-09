import Foundation

public struct Artifact: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public var path: String
    public var contentHash: String
    public var previewPath: String?
    public var model: String
    public var modelID: String
    public var runtime: String
    public var capability: Capability
    public var params: JSONValue
    public var createdAt: Date
    public var durationMs: Int
    public var jobID: String
    public var sessionID: String?

    public init(
        id: String,
        path: String,
        contentHash: String,
        previewPath: String? = nil,
        model: String,
        modelID: String,
        runtime: String,
        capability: Capability,
        params: JSONValue,
        createdAt: Date,
        durationMs: Int,
        jobID: String,
        sessionID: String? = nil
    ) {
        self.id = id
        self.path = path
        self.contentHash = contentHash
        self.previewPath = previewPath
        self.model = model
        self.modelID = modelID
        self.runtime = runtime
        self.capability = capability
        self.params = params
        self.createdAt = createdAt
        self.durationMs = durationMs
        self.jobID = jobID
        self.sessionID = sessionID
    }

    enum CodingKeys: String, CodingKey {
        case id, contentHash, model, modelID, runtime, capability, params
        case createdAt, durationMs, jobID, sessionID
        case path = "artifact"
        case previewPath = "preview"
    }
}

public struct ArtifactDraft: Sendable {
    public var data: Data
    public var fileExtension: String
    public var preview: Data?
    public var model: String
    public var modelID: String
    public var runtime: String
    public var capability: Capability
    public var params: JSONValue
    public var jobID: String
    public var durationMs: Int
    public var sessionID: String?

    public init(
        data: Data,
        fileExtension: String,
        preview: Data? = nil,
        model: String,
        modelID: String,
        runtime: String,
        capability: Capability,
        params: JSONValue,
        jobID: String,
        durationMs: Int,
        sessionID: String? = nil
    ) {
        self.data = data
        self.fileExtension = fileExtension
        self.preview = preview
        self.model = model
        self.modelID = modelID
        self.runtime = runtime
        self.capability = capability
        self.params = params
        self.jobID = jobID
        self.durationMs = durationMs
        self.sessionID = sessionID
    }
}
