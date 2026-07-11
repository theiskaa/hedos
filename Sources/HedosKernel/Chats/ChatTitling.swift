import Foundation

struct ChatTitling: Sendable {
    let chats: ChatStore
    let stream: @Sendable (String, [ChatMessage]) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    >
    let shelf: @Sendable () async throws -> [ModelRecord]

    static let titlingFootprintCeilingMB = 8192

    func autoTitleIfNeeded(sessionID: String) async throws -> String? {
        guard let transcript = try await chats.session(id: sessionID),
            transcript.session.title == ChatSession.defaultTitle
        else { return nil }
        let active = transcript.turns.filter { $0.supersededBy == nil }
        guard let firstUser = active.first(where: { $0.role == .user }) else { return nil }
        guard let reply = active.first(where: { $0.role == .assistant && !$0.content.isEmpty })
        else {
            guard active.contains(where: { $0.role == .assistant && !$0.artifactRefs.isEmpty })
            else { return nil }
            let title = ChatSession.title(from: firstUser.content)
            try await chats.renameSession(id: sessionID, title: title)
            return title
        }
        let title =
            await generatedTitle(
                user: firstUser.content,
                assistant: reply.content,
                boundModelID: transcript.session.modelID)
            ?? ChatSession.title(from: firstUser.content)
        try await chats.renameSession(id: sessionID, title: title)
        return title
    }

    private func generatedTitle(
        user: String, assistant: String, boundModelID: String?
    ) async -> String? {
        guard let modelID = await titlingModelID(bound: boundModelID) else { return nil }
        let prompt = """
            Reply with only a short title, six words at most, naming the topic of this \
            conversation. No quotes, no trailing punctuation.

            User: \(user.prefix(600))
            Assistant: \(assistant.prefix(600))
            """
        guard let upstream = try? await stream(modelID, [ChatMessage(role: .user, content: prompt)])
        else { return nil }
        var text = ""
        do {
            for try await chunk in upstream {
                if case .text(let delta) = chunk {
                    text += delta
                }
                if text.count > 200 { break }
            }
        } catch {
            return nil
        }
        return Self.sanitizedTitle(text)
    }

    private func titlingModelID(bound: String?) async -> String? {
        guard let records = try? await shelf() else { return nil }
        let candidates = records.filter {
            $0.state == .ready && $0.capabilities.contains(.chat)
                && $0.runtime.id != nil && $0.runtime.tier != .recipeNeeded
        }
        guard !candidates.isEmpty else { return nil }
        if let bound,
            let boundRecord = candidates.first(where: { $0.id == bound }),
            boundRecord.footprintMB.map({ $0 <= Self.titlingFootprintCeilingMB }) ?? false
        {
            return boundRecord.id
        }
        return candidates.min {
            ($0.footprintMB ?? .max) < ($1.footprintMB ?? .max)
        }?.id
    }

    static func sanitizedTitle(_ raw: String) -> String? {
        var line =
            raw.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .first { !$0.isEmpty } ?? ""
        line = line.trimmingCharacters(in: CharacterSet(charactersIn: "\"'“”‘’`#*_ "))
        while let last = line.last, ".!,:;".contains(last) {
            line.removeLast()
        }
        let words = line.split(whereSeparator: \.isWhitespace)
        guard !words.isEmpty else { return nil }
        return words.prefix(6).joined(separator: " ")
    }

}
