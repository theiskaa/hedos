import CryptoKit
import Foundation

public enum ChatStoreError: Error, Sendable, LocalizedError, Equatable {
    case databaseUnavailable(String)
    case statementFailed(String)
    case futureSchema(found: Int, supported: Int)
    case sessionNotFound(String)
    case turnNotFound(String)

    public var errorDescription: String? {
        switch self {
        case .databaseUnavailable(let description):
            "Chat database is unavailable: \(description)"
        case .statementFailed(let description):
            "Chat database statement failed: \(description)"
        case .futureSchema(let found, let supported):
            "Chat database schema version \(found) is newer than supported version \(supported)."
        case .sessionNotFound(let id):
            "No chat session with id \(id) is stored."
        case .turnNotFound(let id):
            "No chat turn with id \(id) is stored."
        }
    }
}

public actor ChatStore {
    public let databaseURL: URL

    private var database: ChatDatabase?
    private var queuedWrites: [ChatWrite] = []
    private var shadowSessions: [String: ChatSession] = [:]
    private var shadowTurns: [String: ChatTurn] = [:]
    private var degraded = false

    public init(databaseURL: URL) {
        self.databaseURL = databaseURL
    }

    public static func defaultDatabaseURL() -> URL {
        Registry.defaultDirectory().appendingPathComponent("chats.sqlite")
    }

    public func createSession(
        title: String = ChatSession.defaultTitle, modelID: String? = nil,
        capabilityTags: [String] = [], systemPrompt: String? = nil
    ) throws -> ChatSession {
        let now = Self.now()
        let session = ChatSession(
            id: Self.freshID(),
            title: title,
            createdAt: now,
            updatedAt: now,
            modelID: modelID,
            capabilityTags: capabilityTags,
            systemPrompt: systemPrompt)
        if let database = try writableDatabase() {
            try apply(.insertSession(session), to: database)
        } else {
            shadowSessions[session.id] = session
            queuedWrites.append(.insertSession(session))
        }
        return session
    }

    public func sessions(filter: ChatSessionFilter = .active) throws -> [ChatSession] {
        let database = try open()
        let condition =
            switch filter {
            case .active: "deleted_at IS NULL AND archived = 0"
            case .archived: "deleted_at IS NULL AND archived = 1"
            case .all: "deleted_at IS NULL"
            }
        let rows = try database.rows(
            """
            SELECT id, title, created_at, updated_at, model_id, capability_tags,
                   turn_count, pinned, archived, deleted_at, place, system_prompt, titled_by,
                   intent, image_model_id, voice_model_id
            FROM sessions
            WHERE \(condition)
            ORDER BY updated_at DESC, id
            """)
        return rows.map(Self.session(from:))
    }

    public func usageByDay(since: Date) -> [DayUsage] {
        guard let database = try? open() else { return [] }
        let rows =
            (try? database.rows(
                """
                SELECT t.created_at, t.role, t.stats_json
                FROM turns t JOIN sessions s ON t.session_id = s.id
                WHERE s.deleted_at IS NULL AND t.superseded_by IS NULL
                    AND t.role IN ('user', 'assistant') AND t.created_at >= ?
                """, [.real(since.timeIntervalSince1970)])) ?? []
        var buckets: [Date: (messages: Int, prompt: Int, completion: Int)] = [:]
        let calendar = Calendar.current
        for row in rows {
            let day = calendar.startOfDay(for: Date(timeIntervalSince1970: row.real(0)))
            var entry = buckets[day] ?? (0, 0, 0)
            entry.messages += 1
            if let stats = GenerationStats.fromTurnStatsJSON(row.optionalText(2)) {
                entry.prompt += stats.promptTokens ?? 0
                entry.completion += stats.completionTokens ?? 0
            }
            buckets[day] = entry
        }
        return buckets
            .map {
                DayUsage(
                    day: $0.key, messages: $0.value.messages,
                    promptTokens: $0.value.prompt, completionTokens: $0.value.completion)
            }
            .sorted { $0.day < $1.day }
    }

    public func session(id: String) throws -> ChatTranscript? {
        let database = try open()
        return try database.readTransaction {
            guard
                let row = try database.rows(
                    """
                    SELECT id, title, created_at, updated_at, model_id, capability_tags,
                           turn_count, pinned, archived, deleted_at, place,
                           system_prompt, titled_by, intent, image_model_id, voice_model_id
                    FROM sessions
                    WHERE id = ? AND deleted_at IS NULL
                    """, [.text(id)]
                ).first
            else { return nil }
            let turnRows = try database.rows(
                """
                SELECT id, session_id, seq, role, content, thinking, model_id, stats_json,
                       artifact_refs, superseded_by, content_hash, created_at, updated_at,
                       tool_calls_json, tool_call_id, tool_name, interrupted, attachment_refs
                FROM turns
                WHERE session_id = ?
                ORDER BY seq, created_at, id
                """, [.text(id)])
            return ChatTranscript(
                session: Self.session(from: row), turns: turnRows.map(Self.turn(from:)))
        }
    }

    public func artifactOwners() throws -> [String: String] {
        let database = try open()
        let rows = try database.rows(
            """
            SELECT turns.artifact_refs, turns.session_id
            FROM turns
            JOIN sessions ON sessions.id = turns.session_id
            WHERE sessions.deleted_at IS NULL
              AND turns.artifact_refs <> ''
              AND turns.superseded_by IS NULL
            """)
        var owners: [String: String] = [:]
        for row in rows {
            for ref in Self.splitTags(row.text(0)) {
                owners[ref] = row.text(1)
            }
        }
        return owners
    }

    public func appendTurn(
        _ draft: TurnDraft, to sessionID: String, mergingCapabilityTags: [String] = []
    ) throws -> ChatTurn {
        let now = Self.now()
        if let database = try writableDatabase() {
            return try database.transaction {
                guard
                    let row = try database.rows(
                        "SELECT turn_count FROM sessions WHERE id = ? AND deleted_at IS NULL",
                        [.text(sessionID)]
                    ).first
                else { throw ChatStoreError.sessionNotFound(sessionID) }
                let turn = Self.turn(
                    from: draft, sessionID: sessionID, seq: Int(row.integer(0)), at: now)
                try apply(.insertTurn(turn, mergeTags: mergingCapabilityTags), to: database)
                return turn
            }
        }
        guard var session = shadowSessions[sessionID], session.deletedAt == nil else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        let turn = Self.turn(from: draft, sessionID: sessionID, seq: session.turnCount, at: now)
        session.turnCount += 1
        session.updatedAt = now
        session.capabilityTags = Self.mergedTags(session.capabilityTags, mergingCapabilityTags)
        shadowSessions[sessionID] = session
        shadowTurns[turn.id] = turn
        queuedWrites.append(.insertTurn(turn, mergeTags: mergingCapabilityTags))
        return turn
    }

    public func updateTurn(
        _ turn: ChatTurn, mergingCapabilityTags: [String] = []
    ) throws -> ChatTurn {
        let hash = Self.contentHash(
            content: turn.content, thinking: turn.thinking, modelID: turn.modelID,
            statsJSON: turn.statsJSON, artifactRefs: turn.artifactRefs,
            supersededBy: turn.supersededBy,
            toolCallsJSON: turn.toolCallsJSON, toolCallID: turn.toolCallID,
            toolName: turn.toolName, interrupted: turn.interrupted,
            attachmentRefs: turn.attachmentRefs)
        if let database = try writableDatabase() {
            return try database.transaction {
                guard
                    let row = try database.rows(
                        """
                        SELECT turns.content_hash
                        FROM turns
                        JOIN sessions ON sessions.id = turns.session_id
                        WHERE turns.id = ? AND sessions.deleted_at IS NULL
                        """, [.text(turn.id)]
                    ).first
                else { throw ChatStoreError.turnNotFound(turn.id) }
                guard row.text(0) != hash else { return turn }
                var updated = turn
                updated.contentHash = hash
                updated.updatedAt = Self.now()
                try apply(.updateTurn(updated, mergeTags: mergingCapabilityTags), to: database)
                return updated
            }
        }
        guard
            let stored = shadowTurns[turn.id],
            shadowSessions[stored.sessionID]?.deletedAt == nil
        else {
            throw ChatStoreError.turnNotFound(turn.id)
        }
        guard stored.contentHash != hash else { return stored }
        var updated = turn
        updated.contentHash = hash
        updated.updatedAt = Self.now()
        shadowTurns[turn.id] = updated
        if var session = shadowSessions[turn.sessionID] {
            session.updatedAt = updated.updatedAt
            session.capabilityTags = Self.mergedTags(
                session.capabilityTags, mergingCapabilityTags)
            shadowSessions[turn.sessionID] = session
        }
        queuedWrites.append(.updateTurn(updated, mergeTags: mergingCapabilityTags))
        return updated
    }

    public func renameSession(id: String, title: String, titledBy: String? = nil) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now,
            write: .renameSession(id: id, title: title, titledBy: titledBy, at: now)
        ) {
            $0.title = title
            $0.titledBy = titledBy
        }
    }

    public func rebindSession(id: String, modelID: String?) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now, write: .rebindSession(id: id, modelID: modelID, at: now)
        ) {
            $0.modelID = modelID
        }
    }

    public func setIntent(id: String, intent: ChatIntent) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now, write: .setIntent(id: id, intent: intent, at: now)
        ) {
            $0.intent = intent
        }
    }

    public func bindImageModel(id: String, modelID: String?) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now, write: .bindImageModel(id: id, modelID: modelID, at: now)
        ) {
            $0.imageModelID = modelID
        }
    }

    public func bindVoiceModel(id: String, modelID: String?) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now, write: .bindVoiceModel(id: id, modelID: modelID, at: now)
        ) {
            $0.voiceModelID = modelID
        }
    }

    public func setSystemPrompt(id: String, prompt: String?) throws {
        let now = Self.now()
        try mutateSession(
            id: id, at: now, write: .setSystemPrompt(id: id, prompt: prompt, at: now)
        ) {
            $0.systemPrompt = prompt
        }
    }

    public func setPinned(id: String, _ pinned: Bool) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .setPinned(id: id, pinned: pinned, at: now)) {
            $0.pinned = pinned
        }
    }

    public func setArchived(id: String, _ archived: Bool) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .setArchived(id: id, archived: archived, at: now)) {
            $0.archived = archived
        }
    }

    public func setPlace(id: String, place: String?) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .setPlace(id: id, place: place, at: now)) {
            $0.place = place
        }
    }

    public func deleteSession(id: String) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .tombstoneSession(id: id, at: now)) {
            $0.deletedAt = now
        }
    }

    public func importTranscript(_ transcript: ChatTranscript) throws -> ChatSession {
        let database = try open()
        let collidesWithActive =
            try database.rows(
                "SELECT 1 FROM sessions WHERE id = ? AND deleted_at IS NULL",
                [.text(transcript.session.id)]
            ).first != nil
        let resolved = collidesWithActive ? Self.remapped(transcript) : transcript
        var session = resolved.session
        session.turnCount = resolved.turns.count
        try database.transaction {
            try apply(.insertSession(session), to: database)
            try database.run(
                "DELETE FROM turns WHERE session_id = ?", [.text(session.id)])
            for turn in resolved.turns {
                try database.run(
                    """
                    INSERT INTO turns
                        (id, session_id, seq, role, content, thinking, model_id, stats_json,
                         artifact_refs, superseded_by, content_hash, created_at, updated_at,
                         tool_calls_json, tool_call_id, tool_name, interrupted, attachment_refs)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    [
                        .text(turn.id),
                        .text(turn.sessionID),
                        .integer(Int64(turn.seq)),
                        .text(turn.role.rawValue),
                        .text(turn.content),
                        turn.thinking.map(SQLiteValue.text) ?? .null,
                        turn.modelID.map(SQLiteValue.text) ?? .null,
                        turn.statsJSON.map(SQLiteValue.text) ?? .null,
                        .text(turn.artifactRefs.joined(separator: ",")),
                        turn.supersededBy.map(SQLiteValue.text) ?? .null,
                        .text(turn.contentHash),
                        .real(turn.createdAt.timeIntervalSince1970),
                        .real(turn.updatedAt.timeIntervalSince1970),
                        turn.toolCallsJSON.map(SQLiteValue.text) ?? .null,
                        turn.toolCallID.map(SQLiteValue.text) ?? .null,
                        turn.toolName.map(SQLiteValue.text) ?? .null,
                        .integer(turn.interrupted ? 1 : 0),
                        .text(turn.attachmentRefs.joined(separator: ",")),
                    ])
            }
        }
        return session
    }

    static func remapped(_ transcript: ChatTranscript) -> ChatTranscript {
        let newSessionID = freshID()
        var idMap: [String: String] = [:]
        for turn in transcript.turns { idMap[turn.id] = freshID() }
        let old = transcript.session
        let session = ChatSession(
            id: newSessionID, title: old.title, createdAt: old.createdAt,
            updatedAt: old.updatedAt, modelID: old.modelID, capabilityTags: old.capabilityTags,
            turnCount: old.turnCount, pinned: old.pinned, archived: old.archived, deletedAt: nil,
            place: old.place, systemPrompt: old.systemPrompt, titledBy: old.titledBy,
            intent: old.intent, imageModelID: old.imageModelID, voiceModelID: old.voiceModelID)
        let turns = transcript.turns.map { turn -> ChatTurn in
            let supersededBy = turn.supersededBy.map { idMap[$0] ?? $0 }
            return ChatTurn(
                id: idMap[turn.id] ?? freshID(), sessionID: newSessionID, seq: turn.seq,
                role: turn.role, content: turn.content, thinking: turn.thinking,
                modelID: turn.modelID, statsJSON: turn.statsJSON, artifactRefs: turn.artifactRefs,
                attachmentRefs: turn.attachmentRefs,
                supersededBy: supersededBy,
                contentHash: contentHash(
                    content: turn.content, thinking: turn.thinking, modelID: turn.modelID,
                    statsJSON: turn.statsJSON, artifactRefs: turn.artifactRefs,
                    supersededBy: supersededBy, toolCallsJSON: turn.toolCallsJSON,
                    toolCallID: turn.toolCallID, toolName: turn.toolName,
                    interrupted: turn.interrupted, attachmentRefs: turn.attachmentRefs),
                createdAt: turn.createdAt, updatedAt: turn.updatedAt,
                toolCallsJSON: turn.toolCallsJSON, toolCallID: turn.toolCallID,
                toolName: turn.toolName, interrupted: turn.interrupted)
        }
        return ChatTranscript(session: session, turns: turns)
    }

    public func searchChats(query: String, limit: Int = 50) throws -> [SearchHit] {
        let database = try open()
        let match = Self.ftsQuery(query)
        guard !match.isEmpty else { return [] }
        let rows = try database.rows(
            """
            SELECT turns_fts.session_id, turns_fts.turn_id, sessions.title,
                   snippet(turns_fts, 0, '[', ']', '…', 12), turns_fts.rank
            FROM turns_fts
            JOIN sessions ON sessions.id = turns_fts.session_id
            WHERE turns_fts MATCH ? AND sessions.deleted_at IS NULL
            ORDER BY turns_fts.rank
            LIMIT ?
            """,
            [.text(match), .integer(Int64(limit))])
        return rows.map {
            SearchHit(
                sessionID: $0.text(0),
                turnID: $0.text(1),
                sessionTitle: $0.text(2),
                snippet: $0.text(3),
                rank: $0.real(4))
        }
    }

    func resetWriteCounter() throws {
        try open().resetWriteCounter()
    }

    func rowsWritten() throws -> Int {
        try open().rowsWritten(to: ["sessions", "turns"])
    }

    func enableStatementLogging() throws {
        try open().statementLoggingEnabled = true
    }

    func resetStatementLog() throws {
        try open().resetStatementLog()
    }

    func statementLog() throws -> [String] {
        try open().statementLog
    }

    private func mutateSession(
        id: String, at date: Date, write: ChatWrite, change: (inout ChatSession) -> Void
    ) throws {
        if let database = try writableDatabase() {
            let live = try database.rows(
                "SELECT 1 FROM sessions WHERE id = ? AND deleted_at IS NULL", [.text(id)]
            ).first
            guard live != nil else {
                throw ChatStoreError.sessionNotFound(id)
            }
            try apply(write, to: database)
            return
        }
        guard var session = shadowSessions[id], session.deletedAt == nil else {
            throw ChatStoreError.sessionNotFound(id)
        }
        change(&session)
        session.updatedAt = date
        shadowSessions[id] = session
        queuedWrites.append(write)
    }

    public func appendGeneratedTurn(
        prompt: String, artifactID: String, capabilityTag: String, to sessionID: String
    ) throws {
        guard try session(id: sessionID) != nil else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        _ = try appendTurn(TurnDraft(role: .user, content: prompt), to: sessionID)
        _ = try appendTurn(
            TurnDraft(role: .assistant, content: "", artifactRefs: [artifactID]),
            to: sessionID,
            mergingCapabilityTags: [capabilityTag])
    }

    public func persistenceDegraded() -> Bool {
        degraded
    }

    private func writableDatabase() throws -> ChatDatabase? {
        do {
            return try open()
        } catch ChatStoreError.databaseUnavailable {
            degraded = true
            return nil
        }
    }

    private func open() throws -> ChatDatabase {
        let database: ChatDatabase
        if let existing = self.database {
            database = existing
        } else {
            let fresh = try ChatDatabase(url: databaseURL)
            try fresh.execute("PRAGMA journal_mode=WAL")
            try fresh.execute("PRAGMA foreign_keys=ON")
            try migrate(fresh)
            self.database = fresh
            database = fresh
        }
        if !queuedWrites.isEmpty {
            try flushQueuedWrites(into: database)
        }
        return database
    }

    private func flushQueuedWrites(into database: ChatDatabase) throws {
        while let write = queuedWrites.first {
            try database.transaction { try apply(write, to: database) }
            queuedWrites.removeFirst()
        }
        shadowSessions = [:]
        shadowTurns = [:]
        degraded = false
    }

}
