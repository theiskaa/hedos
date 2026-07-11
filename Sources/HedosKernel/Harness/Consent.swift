import Foundation

public struct ConsentRequest: Sendable, Hashable {
    public enum Kind: Sendable, Hashable {
        case write(path: String, diff: String, overwritesForeignFile: String?)
        case command(argv: [String], timeoutSeconds: Int)
    }

    public let id: String
    public let sessionID: String
    public let toolName: String
    public let kind: Kind

    public init(id: String, sessionID: String, toolName: String, kind: Kind) {
        self.id = id
        self.sessionID = sessionID
        self.toolName = toolName
        self.kind = kind
    }
}

public enum ConsentDecision: Sendable, Hashable {
    case approved(dontAskAgain: Bool)
    case declined
}

public typealias ConsentAsk = @Sendable (ConsentRequest) async -> ConsentDecision

public let alwaysDeclineConsent: ConsentAsk = { _ in .declined }

public actor HarnessActState {
    private var createdFiles: [String: Set<String>] = [:]
    private var grants: [String: Set<String>] = [:]

    public init() {}

    func recordCreated(_ path: String, session: String) {
        createdFiles[session, default: []].insert(path)
    }

    func wasCreatedThisSession(_ path: String, session: String) -> Bool {
        createdFiles[session]?.contains(path) ?? false
    }

    func grant(_ toolName: String, session: String) {
        grants[session, default: []].insert(toolName)
    }

    func isGranted(_ toolName: String, session: String) -> Bool {
        grants[session]?.contains(toolName) ?? false
    }
}

public struct HarnessActContext: Sendable {
    public let sessionID: String
    public let ask: ConsentAsk
    public let state: HarnessActState

    public init(sessionID: String, ask: @escaping ConsentAsk, state: HarnessActState) {
        self.sessionID = sessionID
        self.ask = ask
        self.state = state
    }
}
