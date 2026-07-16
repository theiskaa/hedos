import Foundation

public enum RemovalError: Error, Sendable, Hashable, LocalizedError {
    case notDeletable(kind: SourceKind)
    case modelBusy(name: String)
    case stillDownloading(name: String)
    case daemonUnavailable(hint: String)
    case daemonDeleteFailed(String)
    case trashFailed(path: String, reason: String)

    public var errorDescription: String? {
        switch self {
        case .notDeletable(let kind):
            switch kind {
            case .builtin:
                "Apple's built-in model ships with macOS and can't be deleted."
            case .endpoint:
                "Server models are connections, not files. Remove them from the Servers list."
            default:
                "\(kind.rawValue) models can't be deleted."
            }
        case .modelBusy(let name):
            "\(name) is answering right now. Stop generation, then delete."
        case .stillDownloading(let name):
            "\(name) is still downloading. Cancel the download first, then delete."
        case .daemonUnavailable(let hint):
            hint
        case .daemonDeleteFailed(let message):
            message
        case .trashFailed(let path, let reason):
            "Couldn't move \(path) to the Trash: \(reason)"
        }
    }
}
