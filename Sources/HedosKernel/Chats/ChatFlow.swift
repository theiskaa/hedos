import Foundation

struct ChatFlow: Sendable {
    let chats: ChatStore
    let stream: @Sendable (String, [ChatMessage]) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    >
    let shelf: @Sendable () async throws -> [ModelRecord]

    static let titlingFootprintCeilingMB = 8192
    static let persistCadence: Duration = .milliseconds(250)

    func send(sessionID: String, text: String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        _ = try await chats.appendTurn(TurnDraft(role: .user, content: text), to: sessionID)
        var history = Self.messages(from: transcript.turns)
        history.append(ChatMessage(role: .user, content: text))
        return try await run(session: transcript.session, history: history)
    }

    func continueSession(sessionID: String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        return try await run(
            session: transcript.session, history: Self.messages(from: transcript.turns))
    }

    func autoTitleIfNeeded(sessionID: String) async throws -> String? {
        guard let transcript = try await chats.session(id: sessionID),
            transcript.session.title == ChatSession.defaultTitle
        else { return nil }
        let active = transcript.turns.filter { $0.supersededBy == nil }
        guard let firstUser = active.first(where: { $0.role == .user }),
            let reply = active.first(where: { $0.role == .assistant && !$0.content.isEmpty })
        else { return nil }
        let title =
            await generatedTitle(
                user: firstUser.content,
                assistant: reply.content,
                boundModelID: transcript.session.modelID)
            ?? ChatSession.title(from: firstUser.content)
        try await chats.renameSession(id: sessionID, title: title)
        return title
    }

    private func run(
        session: ChatSession, history: [ChatMessage]
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let modelID = session.modelID else {
            throw KernelError.runtimeFailed("No model is bound to this chat.")
        }
        let upstream = try await stream(modelID, history)
        let chats = chats
        let sessionID = session.id
        return AsyncThrowingStream { continuation in
            let task = Task {
                var turn: ChatTurn?
                var content = ""
                var thinking = ""
                var stats: GenerationStats?
                let clock = ContinuousClock()
                var lastPersist = clock.now

                func persist() async {
                    guard !content.isEmpty || !thinking.isEmpty || stats != nil else { return }
                    if var updated = turn {
                        updated.content = content
                        updated.thinking = thinking.isEmpty ? nil : thinking
                        updated.statsJSON = stats?.turnStatsJSON
                        turn = (try? await chats.updateTurn(updated)) ?? updated
                    } else {
                        turn = try? await chats.appendTurn(
                            TurnDraft(
                                role: .assistant,
                                content: content,
                                thinking: thinking.isEmpty ? nil : thinking,
                                modelID: modelID,
                                stats: stats),
                            to: sessionID)
                    }
                }

                do {
                    for try await chunk in upstream {
                        switch chunk {
                        case .text(let delta):
                            content += delta
                        case .thinking(let delta):
                            thinking += delta
                        case .done(let generationStats):
                            stats = generationStats
                        default:
                            break
                        }
                        continuation.yield(chunk)
                        if turn == nil || clock.now - lastPersist > Self.persistCadence {
                            await persist()
                            lastPersist = clock.now
                        }
                    }
                    await persist()
                    continuation.finish()
                } catch {
                    await persist()
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
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

    private static func messages(from turns: [ChatTurn]) -> [ChatMessage] {
        turns.compactMap { turn in
            guard turn.supersededBy == nil,
                let role = ChatMessage.Role(rawValue: turn.role.rawValue),
                !turn.content.isEmpty
            else { return nil }
            return ChatMessage(role: role, content: turn.content)
        }
    }
}
