import CryptoKit
import Foundation

extension ChatStore {
    enum ChatWrite {
        case insertSession(ChatSession)
        case insertTurn(ChatTurn, mergeTags: [String])
        case updateTurn(ChatTurn, mergeTags: [String])
        case renameSession(id: String, title: String, titledBy: String?, at: Date)
        case setPlace(id: String, place: String?, at: Date)
        case rebindSession(id: String, modelID: String?, at: Date)
        case setIntent(id: String, intent: ChatIntent, at: Date)
        case bindImageModel(id: String, modelID: String?, at: Date)
        case bindVoiceModel(id: String, modelID: String?, at: Date)
        case setSystemPrompt(id: String, prompt: String?, at: Date)
        case setPinned(id: String, pinned: Bool, at: Date)
        case setArchived(id: String, archived: Bool, at: Date)
        case tombstoneSession(id: String, at: Date)
    }

    func apply(_ write: ChatWrite, to database: ChatDatabase) throws {
        switch write {
        case .insertSession(let session):
            try database.run(
                """
                INSERT OR REPLACE INTO sessions
                    (id, title, created_at, updated_at, model_id, capability_tags,
                     turn_count, pinned, archived, deleted_at, place, system_prompt, titled_by,
                     intent, image_model_id, voice_model_id)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                    session.place.map(SQLiteValue.text) ?? .null,
                    session.systemPrompt.map(SQLiteValue.text) ?? .null,
                    session.titledBy.map(SQLiteValue.text) ?? .null,
                    .text(session.intent.rawValue),
                    session.imageModelID.map(SQLiteValue.text) ?? .null,
                    session.voiceModelID.map(SQLiteValue.text) ?? .null,
                ])
        case .insertTurn(let turn, let mergeTags):
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
                    superseded_by = ?, content_hash = ?, updated_at = ?,
                    tool_calls_json = ?, tool_call_id = ?, tool_name = ?, interrupted = ?,
                    attachment_refs = ?
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
                    turn.toolCallsJSON.map(SQLiteValue.text) ?? .null,
                    turn.toolCallID.map(SQLiteValue.text) ?? .null,
                    turn.toolName.map(SQLiteValue.text) ?? .null,
                    .integer(turn.interrupted ? 1 : 0),
                    .text(turn.attachmentRefs.joined(separator: ",")),
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
        case .renameSession(let id, let title, let titledBy, let at):
            try database.run(
                "UPDATE sessions SET title = ?, titled_by = ?, updated_at = ? WHERE id = ?",
                [
                    .text(title),
                    titledBy.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .setPlace(let id, let place, let at):
            try database.run(
                "UPDATE sessions SET place = ?, updated_at = ? WHERE id = ?",
                [
                    place.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .rebindSession(let id, let modelID, let at):
            try database.run(
                "UPDATE sessions SET model_id = ?, updated_at = ? WHERE id = ?",
                [
                    modelID.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .setIntent(let id, let intent, let at):
            try database.run(
                "UPDATE sessions SET intent = ?, updated_at = ? WHERE id = ?",
                [.text(intent.rawValue), .real(at.timeIntervalSince1970), .text(id)])
        case .bindImageModel(let id, let modelID, let at):
            try database.run(
                "UPDATE sessions SET image_model_id = ?, updated_at = ? WHERE id = ?",
                [
                    modelID.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .bindVoiceModel(let id, let modelID, let at):
            try database.run(
                "UPDATE sessions SET voice_model_id = ?, updated_at = ? WHERE id = ?",
                [
                    modelID.map(SQLiteValue.text) ?? .null,
                    .real(at.timeIntervalSince1970),
                    .text(id),
                ])
        case .setSystemPrompt(let id, let prompt, let at):
            try database.run(
                "UPDATE sessions SET system_prompt = ?, updated_at = ? WHERE id = ?",
                [
                    prompt.map(SQLiteValue.text) ?? .null,
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

    static func session(from row: SQLiteRow) -> ChatSession {
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
            deletedAt: row.optionalReal(9).map(Date.init(timeIntervalSince1970:)),
            place: row.optionalText(10),
            systemPrompt: row.optionalText(11),
            titledBy: row.optionalText(12),
            intent: ChatIntent(rawValue: row.text(13)) ?? .text,
            imageModelID: row.optionalText(14),
            voiceModelID: row.optionalText(15))
    }

    static func turn(from row: SQLiteRow) -> ChatTurn {
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
            attachmentRefs: splitTags(row.text(17)),
            supersededBy: row.optionalText(9),
            contentHash: row.text(10),
            createdAt: Date(timeIntervalSince1970: row.real(11)),
            updatedAt: Date(timeIntervalSince1970: row.real(12)),
            toolCallsJSON: row.optionalText(13),
            toolCallID: row.optionalText(14),
            toolName: row.optionalText(15),
            interrupted: row.integer(16) != 0)
    }

    static func turn(
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
            attachmentRefs: draft.attachmentRefs,
            contentHash: contentHash(
                content: draft.content, thinking: draft.thinking, modelID: draft.modelID,
                statsJSON: draft.statsJSON, artifactRefs: draft.artifactRefs, supersededBy: nil,
                toolCallsJSON: draft.toolCallsJSON, toolCallID: draft.toolCallID,
                toolName: draft.toolName, attachmentRefs: draft.attachmentRefs),
            createdAt: date,
            updatedAt: date,
            toolCallsJSON: draft.toolCallsJSON,
            toolCallID: draft.toolCallID,
            toolName: draft.toolName)
    }

    static func contentHash(
        content: String, thinking: String?, modelID: String?, statsJSON: String?,
        artifactRefs: [String], supersededBy: String?,
        toolCallsJSON: String?, toolCallID: String?, toolName: String?,
        interrupted: Bool = false, attachmentRefs: [String] = []
    ) -> String {
        var fields = [
            content,
            thinking ?? "",
            modelID ?? "",
            statsJSON ?? "",
            artifactRefs.joined(separator: ","),
            supersededBy ?? "",
        ]
        if toolCallsJSON != nil || toolCallID != nil || toolName != nil {
            fields.append(toolCallsJSON ?? "")
            fields.append(toolCallID ?? "")
            fields.append(toolName ?? "")
        }
        if interrupted { fields.append("interrupted") }
        if !attachmentRefs.isEmpty { fields.append(attachmentRefs.joined(separator: ",")) }
        let joined = fields.joined(separator: "\u{1f}")
        return SHA256.hash(data: Data(joined.utf8))
            .map { String(format: "%02x", $0) }
            .joined()
    }

    static func mergedTags(_ existing: [String], _ incoming: [String]) -> [String] {
        var merged = existing
        for tag in incoming where !merged.contains(tag) {
            merged.append(tag)
        }
        return merged
    }

    static func splitTags(_ joined: String) -> [String] {
        joined.split(separator: ",").map(String.init)
    }

    static func ftsQuery(_ raw: String) -> String {
        raw.split(whereSeparator: \.isWhitespace)
            .map { "\"\($0.replacingOccurrences(of: "\"", with: "\"\""))\"*" }
            .joined(separator: " ")
    }

    static func freshID() -> String {
        UUID().uuidString.lowercased()
    }

    static func now() -> Date {
        Date.millisecondRounded()
    }
}
