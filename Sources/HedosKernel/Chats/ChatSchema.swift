import Foundation

extension ChatStore {
    func migrate(_ database: ChatDatabase) throws {
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

    static let migrations: [String] = [
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
        """,
        """
        ALTER TABLE turns ADD COLUMN tool_calls_json TEXT;
        ALTER TABLE turns ADD COLUMN tool_call_id TEXT;
        ALTER TABLE turns ADD COLUMN tool_name TEXT;
        """,
        """
        ALTER TABLE sessions ADD COLUMN place TEXT;
        """,
        """
        ALTER TABLE turns ADD COLUMN interrupted INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE sessions ADD COLUMN system_prompt TEXT;
        ALTER TABLE sessions ADD COLUMN titled_by TEXT;
        """,
    ]
}
