import Foundation

public struct PipelineStage: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var modelID: String
    public var capability: Capability
    public var params: [String: JSONValue]

    public init(
        id: String = UUID().uuidString.lowercased(),
        modelID: String,
        capability: Capability,
        params: [String: JSONValue] = [:]
    ) {
        self.id = id
        self.modelID = modelID
        self.capability = capability
        self.params = params
    }
}

public struct Pipeline: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var name: String
    public var stages: [PipelineStage]
    public var createdAt: Date
    public var updatedAt: Date

    public init(
        id: String = UUID().uuidString.lowercased(),
        name: String,
        stages: [PipelineStage],
        createdAt: Date = Date(),
        updatedAt: Date = Date()
    ) {
        self.id = id
        self.name = name
        self.stages = stages
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }
}

public enum PipelineInput: Sendable {
    case audio([Float])
    case text(String)
}
