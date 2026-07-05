import Foundation

public enum JobState: String, Codable, Hashable, Sendable {
    case queued
    case preparing
    case running
    case done
    case failed
    case cancelled

    public var isTerminal: Bool {
        switch self {
        case .done, .failed, .cancelled: true
        case .queued, .preparing, .running: false
        }
    }
}

public struct JobProgress: Codable, Hashable, Sendable {
    public var fraction: Double
    public var step: Int?
    public var totalSteps: Int?

    public init(fraction: Double = 0, step: Int? = nil, totalSteps: Int? = nil) {
        self.fraction = fraction
        self.step = step
        self.totalSteps = totalSteps
    }

    public static let none = JobProgress()
}

public struct Job: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public let modelID: String
    public let capability: Capability
    public let payload: JSONValue
    public var state: JobState
    public var progress: JobProgress
    public var queueReason: String?
    public var preview: Data? = nil
    public var result: [String]
    public var error: String?
    public var submittedAt: Date
    public var startedAt: Date?
    public var finishedAt: Date?

    public init(
        id: String = UUID().uuidString,
        modelID: String,
        capability: Capability,
        payload: JSONValue,
        state: JobState = .queued,
        progress: JobProgress = .none,
        queueReason: String? = nil,
        preview: Data? = nil,
        result: [String] = [],
        error: String? = nil,
        submittedAt: Date = Date(),
        startedAt: Date? = nil,
        finishedAt: Date? = nil
    ) {
        self.id = id
        self.modelID = modelID
        self.capability = capability
        self.payload = payload
        self.state = state
        self.progress = progress
        self.queueReason = queueReason
        self.preview = preview
        self.result = result
        self.error = error
        self.submittedAt = submittedAt
        self.startedAt = startedAt
        self.finishedAt = finishedAt
    }

    enum CodingKeys: String, CodingKey {
        case id, modelID, capability, payload, state, progress, queueReason
        case result, error, submittedAt, startedAt, finishedAt
    }
}
