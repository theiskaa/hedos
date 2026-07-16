import Foundation

public struct InstallProgress: Sendable, Hashable {
    public var bytesDownloaded: Int64
    public var totalBytes: Int64?
    public var totalIsPartial: Bool
    public var currentFile: String?

    public init(
        bytesDownloaded: Int64 = 0, totalBytes: Int64? = nil, totalIsPartial: Bool = false,
        currentFile: String? = nil
    ) {
        self.bytesDownloaded = bytesDownloaded
        self.totalBytes = totalBytes
        self.totalIsPartial = totalIsPartial
        self.currentFile = currentFile
    }

    public var fraction: Double? {
        guard let totalBytes, totalBytes > 0, !totalIsPartial else { return nil }
        return min(max(Double(bytesDownloaded) / Double(totalBytes), 0), 1)
    }
}

public enum InstallEvent: Sendable, Hashable {
    case queued
    case preparing
    case status(String)
    case progress(InstallProgress)
    case done
    case failed(message: String)
    case cancelled

    public var isTerminal: Bool {
        switch self {
        case .done, .failed, .cancelled: true
        case .queued, .preparing, .status, .progress: false
        }
    }
}

public enum InstallStreamEvent: Sendable, Hashable {
    case status(String)
    case progress(InstallProgress)
}
