import Foundation

public enum JobEvent: Hashable, Sendable {
    case queued(reason: String?)
    case preparing
    case status(String)
    case running
    case progress(JobProgress)
    case preview(Data)
    case done(result: [String])
    case failed(message: String)
    case cancelled
}

public enum JobRuntimeEvent: Hashable, Sendable {
    case status(String)
    case started
    case progress(step: Int, totalSteps: Int)
    case preview(Data)
    case result(data: Data, fileExtension: String)
    case artifacts([String])
}
