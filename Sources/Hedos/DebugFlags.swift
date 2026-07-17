import Foundation

enum DebugFlags {
    #if DEBUG
        static let forceEmpty: Bool = {
            let env = ProcessInfo.processInfo.environment
            if let raw = env["HEDOS_FORCE_EMPTY"] {
                return raw != "0" && raw.lowercased() != "false"
            }
            return ProcessInfo.processInfo.arguments.contains("--force-empty")
        }()
    #else
        static let forceEmpty = false
    #endif
}
