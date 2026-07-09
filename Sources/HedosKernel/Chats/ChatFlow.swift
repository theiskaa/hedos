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

    func editUserTurn(
        sessionID: String, turnID: String, text: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        let active = transcript.turns.filter { $0.supersededBy == nil }
        guard let index = active.firstIndex(where: { $0.id == turnID }),
            active[index].role == .user
        else { throw ChatStoreError.turnNotFound(turnID) }
        let replacement = try await chats.appendTurn(
            TurnDraft(role: .user, content: text), to: sessionID)
        for turn in active[index...] {
            var retired = turn
            retired.supersededBy = replacement.id
            _ = try await chats.updateTurn(retired)
        }
        var history = Self.messages(from: Array(active[..<index]))
        history.append(ChatMessage(role: .user, content: text))
        return try await run(session: transcript.session, history: history)
    }

    func regenerate(
        sessionID: String, turnID: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        let active = transcript.turns.filter { $0.supersededBy == nil }
        guard let index = active.firstIndex(where: { $0.id == turnID }),
            active[index].role == .assistant
        else { throw ChatStoreError.turnNotFound(turnID) }
        return try await run(
            session: transcript.session,
            history: Self.messages(from: Array(active[..<index])),
            retiring: Array(active[index...]))
    }

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

    private func run(
        session: ChatSession, history: [ChatMessage], retiring: [ChatTurn] = []
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
                var ttftMs: Int?
                let clock = ContinuousClock()
                let started = clock.now
                var lastPersist = started

                func persist() async {
                    guard !content.isEmpty || !thinking.isEmpty || stats != nil else { return }
                    let tags = thinking.isEmpty ? [] : [SessionTag.thinking]
                    var mergedStats = stats
                    if mergedStats != nil, mergedStats?.ttftMs == nil {
                        mergedStats?.ttftMs = ttftMs
                    }
                    if var updated = turn {
                        updated.content = content
                        updated.thinking = thinking.isEmpty ? nil : thinking
                        updated.statsJSON = mergedStats?.turnStatsJSON
                        turn =
                            (try? await chats.updateTurn(updated, mergingCapabilityTags: tags))
                            ?? updated
                    } else {
                        turn = try? await chats.appendTurn(
                            TurnDraft(
                                role: .assistant,
                                content: content,
                                thinking: thinking.isEmpty ? nil : thinking,
                                modelID: modelID,
                                stats: mergedStats),
                            to: sessionID,
                            mergingCapabilityTags: tags)
                        if let replacement = turn {
                            for retired in retiring where retired.supersededBy == nil {
                                var superseded = retired
                                superseded.supersededBy = replacement.id
                                _ = try? await chats.updateTurn(superseded)
                            }
                        }
                    }
                }

                do {
                    for try await chunk in upstream {
                        switch chunk {
                        case .text(let delta):
                            if ttftMs == nil, !delta.isEmpty {
                                ttftMs = Int((clock.now - started) / .milliseconds(1))
                            }
                            content += delta
                        case .thinking(let delta):
                            if ttftMs == nil, !delta.isEmpty {
                                ttftMs = Int((clock.now - started) / .milliseconds(1))
                            }
                            thinking += delta
                        case .done(let generationStats):
                            stats = generationStats ?? GenerationStats()
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

    static func messages(from turns: [ChatTurn]) -> [ChatMessage] {
        let active = turns.filter { $0.supersededBy == nil }
        var messages: [ChatMessage] = []
        var index = 0
        while index < active.count {
            let turn = active[index]
            if turn.role == .user, index + 1 < active.count,
                active[index + 1].isGeneratedArtifact
            {
                index += 2
                continue
            }
            if let role = ChatMessage.Role(rawValue: turn.role.rawValue), !turn.content.isEmpty {
                if let last = messages.last, last.role == role {
                    messages[messages.count - 1] = ChatMessage(
                        role: role, content: last.content + "\n\n" + turn.content)
                } else {
                    messages.append(ChatMessage(role: role, content: turn.content))
                }
            }
            index += 1
        }
        return messages
    }
}
