import CryptoKit
import Foundation

public enum ChatStoreError: Error, Sendable, LocalizedError {
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

    public init(databaseURL: URL) {
        self.databaseURL = databaseURL
    }

    public static func defaultDatabaseURL() -> URL {
        Registry.defaultDirectory().appendingPathComponent("chats.sqlite")
    }

    public func createSession(
        title: String = ChatSession.defaultTitle, modelID: String? = nil,
        capabilityTags: [String] = []
    ) throws -> ChatSession {
        let now = Self.now()
        let session = ChatSession(
            id: Self.freshID(),
            title: title,
            createdAt: now,
            updatedAt: now,
            modelID: modelID,
            capabilityTags: capabilityTags)
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
                   turn_count, pinned, archived, deleted_at
            FROM sessions
            WHERE \(condition)
            ORDER BY updated_at DESC, id
            """)
        return rows.map(Self.session(from:))
    }

    public func session(id: String) throws -> ChatTranscript? {
        let database = try open()
        return try database.readTransaction {
            guard
                let row = try database.rows(
                    """
                    SELECT id, title, created_at, updated_at, model_id, capability_tags,
                           turn_count, pinned, archived, deleted_at
                    FROM sessions
                    WHERE id = ? AND deleted_at IS NULL
                    """, [.text(id)]
                ).first
            else { return nil }
            let turnRows = try database.rows(
                """
                SELECT id, session_id, seq, role, content, thinking, model_id, stats_json,
                       artifact_refs, superseded_by, content_hash, created_at, updated_at
                FROM turns
                WHERE session_id = ?
                ORDER BY seq, created_at, id
                """, [.text(id)])
            return ChatTranscript(
                session: Self.session(from: row), turns: turnRows.map(Self.turn(from:)))
        }
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
            supersededBy: turn.supersededBy)
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

    public func renameSession(id: String, title: String) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .renameSession(id: id, title: title, at: now)) {
            $0.title = title
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

    public func deleteSession(id: String) throws {
        let now = Self.now()
        try mutateSession(id: id, at: now, write: .tombstoneSession(id: id, at: now)) {
            $0.deletedAt = now
        }
    }

    public func importTranscript(_ transcript: ChatTranscript) throws -> ChatSession {
        let database = try open()
        try database.transaction {
            try apply(.insertSession(transcript.session), to: database)
            try database.run(
                "DELETE FROM turns WHERE session_id = ?", [.text(transcript.session.id)])
            for turn in transcript.turns {
                try database.run(
                    """
                    INSERT INTO turns
                        (id, session_id, seq, role, content, thinking, model_id, stats_json,
                         artifact_refs, superseded_by, content_hash, created_at, updated_at)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                    ])
            }
        }
        return transcript.session
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

    public func resetWriteCounter() throws {
        try open().resetWriteCounter()
    }

    public func rowsWritten() throws -> Int {
        try open().rowsWritten(to: ["sessions", "turns"])
    }

    public func resetStatementLog() throws {
        try open().resetStatementLog()
    }

    public func statementLog() throws -> [String] {
        try open().statementLog
    }

    private enum ChatWrite {
        case insertSession(ChatSession)
        case insertTurn(ChatTurn, mergeTags: [String])
        case updateTurn(ChatTurn, mergeTags: [String])
        case renameSession(id: String, title: String, at: Date)
        case rebindSession(id: String, modelID: String?, at: Date)
        case setPinned(id: String, pinned: Bool, at: Date)
        case setArchived(id: String, archived: Bool, at: Date)
        case tombstoneSession(id: String, at: Date)
    }

    private func mutateSession(
        id: String, at date: Date, write: ChatWrite, change: (inout ChatSession) -> Void
    ) throws {
        if let database = try writableDatabase() {
            try apply(write, to: database)
            return
        }
        if var session = shadowSessions[id] {
            change(&session)
            session.updatedAt = date
            shadowSessions[id] = session
        }
        queuedWrites.append(write)
    }

    private func writableDatabase() throws -> ChatDatabase? {
        do {
            return try open()
        } catch ChatStoreError.databaseUnavailable {
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

    private func migrate(_ database: ChatDatabase) throws {
        let version = try database.userVersion()
        guard version <= Self.migrations.count else {
            throw ChatStoreError.futureSchema(found: version, supported: Self.migrations.count)
        }
        guard version < Self.migrations.count else { return }
        try database.transaction {
            let current = try database.userVersion()
            guard current < Self.migrations.count else { return }
            for migration in Self.migrations[current...] {
                try database.execute(migration)
            }
            try database.setUserVersion(Self.migrations.count)
        }
    }

    private func flushQueuedWrites(into database: ChatDatabase) throws {
        while let write = queuedWrites.first {
            try database.transaction { try apply(write, to: database) }
            queuedWrites.removeFirst()
        }
        shadowSessions = [:]
        shadowTurns = [:]
    }

    private func apply(_ write: ChatWrite, to database: ChatDatabase) throws {
        switch write {
        case .insertSession(let session):
            try database.run(
                """
                INSERT OR REPLACE INTO sessions
                    (id, title, created_at, updated_at, model_id, capability_tags,
                     turn_count, pinned, archived, deleted_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                [
                    .text(session.id),
                    .text(session.title),
                    .real(session.createdAt.timeIntervalSince1970),
                    .real(session.updatedAt.timeIntervalSince1970),
                    session.modelID.map(SQLiteValue.text) ?? .null,
                    .text(session.capabilityTags.joined(separator: ",")),
                    .integer(Int64(session.turnCount)),
                    .integer(session.pinned ? 1 : 0),
                    .integer(session.archived ? 1 : 0),
                    session.deletedAt.map { .real($0.timeIntervalSince1970) } ?? .null,
                ])
        case .insertTurn(let turn, let mergeTags):
            try database.run(
                """
                INSERT INTO turns
                    (id, session_id, seq, role, content, thinking, model_id, stats_json,
                     artifact_refs, superseded_by, content_hash, created_at, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                ])
            let storedTags = try database.rows(
                "SELECT capability_tags FROM sessions WHERE id = ?", [.text(turn.sessionID)]
            ).first.map { Self.splitTags($0.text(0)) } ?? []
            try database.run(
                """
                UPDATE sessions
                SET turn_count = turn_count + 1, updated_at = ?, capability_tags = ?
                WHERE id = ?
                """,
                [
                    .real(turn.updatedAt.timeIntervalSince1970),
                    .text(Self.mergedTags(storedTags, mergeTags).joined(separator: ",")),
                    .text(turn.sessionID),
                ])
        case .updateTurn(let turn, let mergeTags):
            try database.run(
                """
                UPDATE turns
                SET content = ?, thinking = ?, model_id = ?, stats_json = ?, artifact_refs = ?,
                    superseded_by = ?, content_hash = ?, updated_at = ?
                WHERE id = ?
                """,
                [
                    .text(turn.content),
                    turn.thinking.map(SQLiteValue.text) ?? .null,
                    turn.modelID.map(SQLiteValue.text) ?? .null,
                    turn.statsJSON.map(SQLiteValue.text) ?? .null,
                    .text(turn.artifactRefs.joined(separator: ",")),
                    turn.supersededBy.map(SQLiteValue.text) ?? .null,
                    .text(turn.contentHash),
                    .real(turn.updatedAt.timeIntervalSince1970),
                    .text(turn.id),
                ])
            let storedTags = try database.rows(
                "SELECT capability_tags FROM sessions WHERE id = ?", [.text(turn.sessionID)]
            ).first.map { Self.splitTags($0.text(0)) } ?? []
            try database.run(
                "UPDATE sessions SET updated_at = ?, capability_tags = ? WHERE id = ?",
                [
                    .real(turn.updatedAt.timeIntervalSince1970),
                    .text(Self.mergedTags(storedTags, mergeTags).joined(separator: ",")),
                    .text(turn.sessionID),
                ])
        case .renameSession(let id, let title, let at):
            try database.run(
                "UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?",
                [.text(title), .real(at.timeIntervalSince1970), .text(id)])
        case .rebindSession(let id, let modelID, let at):
            try database.run(
                "UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?",
                [
                    modelID.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .setPinned(let id, let pinned, let at):
            try database.run(
                "UPDATE sessions SET pinned = ?, updated_at = ? WHERE id = ?",
                [.integer(pinned ? 1 : 0), .real(at.timeIntervalSince1970), .text(id)])
        case .setArchived(let id, let archived, let at):
            try database.run(
                "UPDATE sessions SET archived = ?, updated_at = ? WHERE id = ?",
                [.integer(archived ? 1 : 0), .real(at.timeIntervalSince1970), .text(id)])
        case .tombstoneSession(let id, let at):
            try database.run(
                "UPDATE sessions SET deleted_at = ?, updated_at = ? WHERE id = ? AND deleted_at IS NULL",
                [.real(at.timeIntervalSince1970), .real(at.timeIntervalSince1970), .text(id)])
        }
    }

    private static func session(from row: SQLiteRow) -> ChatSession {
        ChatSession(
            id: row.text(0),
            title: row.text(1),
            createdAt: Date(timeIntervalSince1970: row.real(2)),
            updatedAt: Date(timeIntervalSince1970: row.real(3)),
            modelID: row.optionalText(4),
            capabilityTags: splitTags(row.text(5)),
            turnCount: Int(row.integer(6)),
            pinned: row.integer(7) != 0,
            archived: row.integer(8) != 0,
            deletedAt: row.optionalReal(9).map(Date.init(timeIntervalSince1970:)))
    }

    private static func turn(from row: SQLiteRow) -> ChatTurn {
        ChatTurn(
            id: row.text(0),
            sessionID: row.text(1),
            seq: Int(row.integer(2)),
            role: TurnRole(rawValue: row.text(3)),
            content: row.text(4),
            thinking: row.optionalText(5),
            modelID: row.optionalText(6),
            statsJSON: row.optionalText(7),
            artifactRefs: splitTags(row.text(8)),
            supersededBy: row.optionalText(9),
            contentHash: row.text(10),
            createdAt: Date(timeIntervalSince1970: row.real(11)),
            updatedAt: Date(timeIntervalSince1970: row.real(12)))
    }

    private static func turn(
        from draft: TurnDraft, sessionID: String, seq: Int, at date: Date
    ) -> ChatTurn {
        ChatTurn(
            id: freshID(),
            sessionID: sessionID,
            seq: seq,
            role: draft.role,
            content: draft.content,
            thinking: draft.thinking,
            modelID: draft.modelID,
            statsJSON: draft.statsJSON,
            artifactRefs: draft.artifactRefs,
            contentHash: contentHash(
                content: draft.content, thinking: draft.thinking, modelID: draft.modelID,
                statsJSON: draft.statsJSON, artifactRefs: draft.artifactRefs, supersededBy: nil),
            createdAt: date,
            updatedAt: date)
    }

    private static func contentHash(
        content: String, thinking: String?, modelID: String?, statsJSON: String?,
        artifactRefs: [String], supersededBy: String?
    ) -> String {
        let joined = [
            content,
            thinking ?? "",
            modelID ?? "",
            statsJSON ?? "",
            artifactRefs.joined(separator: ","),
            supersededBy ?? "",
        ].joined(separator: "\u{1f}")
        return SHA256.hash(data: Data(joined.utf8))
            .map { String(format: "%02x", $0) }
            .joined()
    }

    private static func mergedTags(_ existing: [String], _ incoming: [String]) -> [String] {
        var merged = existing
        for tag in incoming where !merged.contains(tag) {
            merged.append(tag)
        }
        return merged
    }

    private static func splitTags(_ joined: String) -> [String] {
        joined.split(separator: ",").map(String.init)
    }

    private static func ftsQuery(_ raw: String) -> String {
        raw.split(whereSeparator: \.isWhitespace)
            .map { "\"\($0.replacingOccurrences(of: "\"", with: "\"\""))\"*" }
            .joined(separator: " ")
    }

    private static func freshID() -> String {
        UUID().uuidString.lowercased()
    }

    private static func now() -> Date {
        Date(timeIntervalSince1970: (Date().timeIntervalSince1970 * 1000).rounded() / 1000)
    }

    private static let migrations: [String] = [
        """
        CREATE TABLE sessions(
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL,
            model_id TEXT,
            capability_tags TEXT NOT NULL DEFAULT '',
            turn_count INTEGER NOT NULL DEFAULT 0,
            pinned INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            deleted_at REAL
        );
        CREATE INDEX sessions_updated_at ON sessions(updated_at DESC);
        CREATE TABLE turns(
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            thinking TEXT,
            model_id TEXT,
            stats_json TEXT,
            artifact_refs TEXT,
            superseded_by TEXT,
            content_hash TEXT NOT NULL,
            created_at REAL NOT NULL,
            updated_at REAL NOT NULL
        );
        CREATE INDEX turns_session_seq ON turns(session_id, seq);
        CREATE VIRTUAL TABLE turns_fts USING fts5(
            content, title, turn_id UNINDEXED, session_id UNINDEXED);
        CREATE TRIGGER turns_fts_insert AFTER INSERT ON turns BEGIN
            INSERT INTO turns_fts(rowid, content, title, turn_id, session_id)
            VALUES (
                new.rowid,
                new.content,
                (SELECT title FROM sessions WHERE id = new.session_id),
                new.id,
                new.session_id);
        END;
        CREATE TRIGGER turns_fts_update AFTER UPDATE OF content ON turns BEGIN
            UPDATE turns_fts SET content = new.content WHERE rowid = new.rowid;
        END;
        CREATE TRIGGER turns_fts_delete AFTER DELETE ON turns BEGIN
            DELETE FROM turns_fts WHERE rowid = old.rowid;
        END;
        CREATE TRIGGER sessions_fts_retitle AFTER UPDATE OF title ON sessions BEGIN
            UPDATE turns_fts SET title = new.title WHERE session_id = new.id;
        END;
        """
    ]
}
