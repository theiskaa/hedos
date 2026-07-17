import Foundation

struct ChatFlow: Sendable {
    let chats: ChatStore
    let stream: @Sendable (String, [ChatMessage], [ToolSpec], String?) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    let shelf: @Sendable () async throws -> [ModelRecord]
    var toolbox: @Sendable (ChatSession) async -> [ToolSpec] = { _ in [] }
    var execute: @Sendable (String, ToolCall) async -> ToolOutcome = { _, _ in
        ToolOutcome(text: "")
    }
    var attachments: AttachmentStore?
    var gate = ChatSessionGate()

    static let persistCadence: Duration = .milliseconds(250)
    static let interruptedMarker = "\n\n[reply interrupted — may be incomplete]"
    static let mergeBoundary = "\n\n---\n\n"
    static let maxToolCallsReadOnly = 8
    static let maxToolCallsActing = 16
    static let maxMentionReadsPerSend = 4
    static let toolResultContextBudgetBytes = 16_384

    func send(
        sessionID: String, text: String, attachments messageAttachments: [ChatAttachment] = []
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try gate.begin(sessionID)
        do {
            return try await sendLocked(
                sessionID: sessionID, text: text, attachments: messageAttachments)
        } catch {
            gate.end(sessionID)
            throw error
        }
    }

    private func sendLocked(
        sessionID: String, text: String, attachments messageAttachments: [ChatAttachment]
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let transcript = try await chats.session(id: sessionID) else {
            throw ChatStoreError.sessionNotFound(sessionID)
        }
        let attachmentRefs: [String]
        if let attachments {
            attachmentRefs = (try? attachments.store(messageAttachments)) ?? []
        } else {
            attachmentRefs = []
        }
        _ = try await chats.appendTurn(
            TurnDraft(role: .user, content: text, attachmentRefs: attachmentRefs), to: sessionID)
        var history = projected(transcript.turns)
        history.append(
            ChatMessage(
                role: .user, content: text, attachments: messageAttachments,
                attachmentRefs: attachmentRefs))

        let mentioned = Self.mentionedFiles(in: text, place: transcript.session.place)
        if !mentioned.isEmpty {
            let calls = mentioned.map { path in
                ToolCall(
                    name: HarnessTools.readFileName,
                    arguments: .object(["path": .string(path)]))
            }
            _ = try await chats.appendTurn(
                TurnDraft(
                    role: .assistant, content: "", modelID: transcript.session.modelID,
                    stats: nil, toolCalls: calls),
                to: sessionID)
            history.append(ChatMessage(role: .assistant, content: "", toolCalls: calls))
            for call in calls {
                let result = Self.truncatedToolResult(await execute(sessionID, call).text)
                _ = try await chats.appendTurn(
                    TurnDraft(
                        role: .tool, content: result, stats: nil,
                        toolCallID: call.id, toolName: call.name),
                    to: sessionID)
                history.append(
                    ChatMessage(
                        role: .tool, content: result,
                        toolCallID: call.id, toolName: call.name))
            }
        }
        return try await run(session: transcript.session, history: history)
    }

    static func mentionedFiles(in text: String, place: String?) -> [String] {
        guard let place else { return [] }
        var mentioned: [String] = []
        for rawToken in text.split(whereSeparator: \.isWhitespace) {
            guard let (token, explicit) = PromptComposer.mentionCore(rawToken) else { continue }
            guard explicit || token.contains("/") || token.contains(".") else { continue }
            guard let resolved = try? PlaceBoundary.resolve(token, in: place) else { continue }
            var isDirectory: ObjCBool = false
            let exists = FileManager.default.fileExists(
                atPath: resolved, isDirectory: &isDirectory)
            if exists, isDirectory.boolValue { continue }
            if !exists, !explicit { continue }
            guard resolved.hasPrefix(place + "/") else { continue }
            let relative = String(resolved.dropFirst(place.count + 1))
            if !mentioned.contains(relative) {
                mentioned.append(relative)
            }
            if mentioned.count >= Self.maxMentionReadsPerSend { break }
        }
        return mentioned
    }

    func continueSession(sessionID: String) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    > {
        try gate.begin(sessionID)
        do {
            guard let transcript = try await chats.session(id: sessionID) else {
                throw ChatStoreError.sessionNotFound(sessionID)
            }
            let history = projected(transcript.turns)
            guard history.last?.role == .user else {
                throw KernelError.runtimeFailed(
                    "Nothing to continue: the conversation already ends with a reply.")
            }
            return try await run(session: transcript.session, history: history)
        } catch {
            gate.end(sessionID)
            throw error
        }
    }

    func editUserTurn(
        sessionID: String, turnID: String, text: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try gate.begin(sessionID)
        do {
            guard let transcript = try await chats.session(id: sessionID) else {
                throw ChatStoreError.sessionNotFound(sessionID)
            }
            let active = transcript.turns.filter { $0.supersededBy == nil }
            guard let index = active.firstIndex(where: { $0.id == turnID }),
                active[index].role == .user
            else { throw ChatStoreError.turnNotFound(turnID) }
            let replacement = try await chats.appendTurn(
                TurnDraft(role: .user, content: text), to: sessionID)
            var history = projected(Array(active[..<index]))
            history.append(ChatMessage(role: .user, content: text))
            return try await run(
                session: transcript.session, history: history,
                retiring: Array(active[index...]), userRetirementID: replacement.id)
        } catch {
            gate.end(sessionID)
            throw error
        }
    }

    func regenerate(
        sessionID: String, turnID: String
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        try gate.begin(sessionID)
        do {
            guard let transcript = try await chats.session(id: sessionID) else {
                throw ChatStoreError.sessionNotFound(sessionID)
            }
            let active = transcript.turns.filter { $0.supersededBy == nil }
            guard let index = active.firstIndex(where: { $0.id == turnID }),
                active[index].role == .assistant
            else { throw ChatStoreError.turnNotFound(turnID) }
            return try await run(
                session: transcript.session,
                history: projected(Array(active[..<index])),
                retiring: Array(active[index...]))
        } catch {
            gate.end(sessionID)
            throw error
        }
    }

    private func run(
        session: ChatSession, history: [ChatMessage], retiring: [ChatTurn] = [],
        userRetirementID: String? = nil
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        guard let modelID = session.modelID else {
            throw KernelError.noBoundModel
        }
        let tools = await toolbox(session)
        let sessionPrompt = session.systemPrompt
        let upstream = try await stream(modelID, history, tools, sessionPrompt)
        let chats = chats
        let stream = stream
        let execute = execute
        let sessionID = session.id
        let gate = gate
        return AsyncThrowingStream { continuation in
            let task = Task {
                var history = history
                var retiring = retiring
                var currentUpstream = upstream
                var offeredTools = tools
                let toolCallCap =
                    Harness.offersActTools(tools)
                    ? Self.maxToolCallsActing : Self.maxToolCallsReadOnly
                var executedCalls = 0
                var turn: ChatTurn?
                var content = ""
                var thinking = ""
                var stats: GenerationStats?
                var toolCalls: [ToolCall] = []
                var ttftMs: Int?
                var pendingCalls: [ToolCall] = []
                var persistFailure: Error?
                let clock = ContinuousClock()
                let started = clock.now
                var lastPersist = started

                func persist() async {
                    guard !content.isEmpty || !thinking.isEmpty || stats != nil
                        || !toolCalls.isEmpty
                    else { return }
                    persistFailure = nil
                    let tags = thinking.isEmpty ? [] : [SessionTag.thinking]
                    var mergedStats = stats
                    if mergedStats != nil, mergedStats?.ttftMs == nil {
                        mergedStats?.ttftMs = ttftMs
                    }
                    do {
                        if var updated = turn {
                            updated.content = content
                            updated.thinking = thinking.isEmpty ? nil : thinking
                            updated.statsJSON = mergedStats?.turnStatsJSON
                            updated.toolCallsJSON = toolCalls.turnToolCallsJSON
                            turn = try await chats.updateTurn(
                                updated, mergingCapabilityTags: tags)
                        } else {
                            let appended = try await chats.appendTurn(
                                TurnDraft(
                                    role: .assistant,
                                    content: content,
                                    thinking: thinking.isEmpty ? nil : thinking,
                                    modelID: modelID,
                                    stats: mergedStats,
                                    toolCalls: toolCalls),
                                to: sessionID,
                                mergingCapabilityTags: tags)
                            turn = appended
                            for retired in retiring where retired.supersededBy == nil {
                                var superseded = retired
                                superseded.supersededBy =
                                    retired.role == .user
                                    ? userRetirementID ?? appended.id : appended.id
                                _ = try await chats.updateTurn(superseded)
                            }
                            retiring = []
                        }
                    } catch {
                        persistFailure = error
                    }
                }

                do {
                    while true {
                        for try await chunk in currentUpstream {
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
                            case .toolCall(let call):
                                toolCalls.append(call)
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
                        guard persistFailure == nil else { break }
                        guard !toolCalls.isEmpty, !offeredTools.isEmpty else { break }

                        let calls = toolCalls
                        pendingCalls = calls
                        var projections: [ChatMessage] = [
                            ChatMessage(
                                role: .assistant, content: content, toolCalls: calls)
                        ]
                        for call in calls {
                            var result: String
                            var producedArtifacts: [String] = []
                            if executedCalls >= toolCallCap {
                                result =
                                    "[skipped: the tool-call limit of "
                                    + "\(toolCallCap) calls for this message "
                                    + "was reached — answer with what you have]"
                            } else {
                                try Task.checkCancellation()
                                continuation.yield(.status(Harness.actionSummary(call)))
                                let outcome = await execute(sessionID, call)
                                result = Self.truncatedToolResult(outcome.text)
                                producedArtifacts = outcome.artifactRefs
                                executedCalls += 1
                                if executedCalls >= toolCallCap {
                                    result +=
                                        "\n\n[tool-call limit reached: "
                                        + "\(toolCallCap) calls for this message — "
                                        + "answer with what you have]"
                                }
                            }
                            let resultTurn = try await chats.appendTurn(
                                TurnDraft(
                                    role: .tool,
                                    content: result,
                                    stats: nil,
                                    artifactRefs: producedArtifacts,
                                    toolCallID: call.id,
                                    toolName: call.name),
                                to: sessionID)
                            pendingCalls.removeAll { $0.id == call.id }
                            projections.append(
                                ChatMessage(
                                    role: .tool, content: result,
                                    toolCallID: resultTurn.toolCallID,
                                    toolName: resultTurn.toolName))
                        }
                        history.append(contentsOf: projections)
                        try Task.checkCancellation()
                        if executedCalls >= toolCallCap {
                            offeredTools = []
                        }
                        turn = nil
                        content = ""
                        thinking = ""
                        stats = nil
                        toolCalls = []
                        ttftMs = nil
                        currentUpstream = try await stream(
                            modelID, history, offeredTools, sessionPrompt)
                    }
                    if Task.isCancelled, stats == nil, var completed = turn,
                        completed.role == .assistant, !completed.interrupted
                    {
                        completed.interrupted = true
                        turn = (try? await chats.updateTurn(completed)) ?? completed
                    }
                    if let persistFailure {
                        continuation.finish(throwing: persistFailure)
                    } else {
                        continuation.finish()
                    }
                } catch {
                    await persist()
                    if var completed = turn, completed.role == .assistant, !completed.interrupted {
                        completed.interrupted = true
                        turn = (try? await chats.updateTurn(completed)) ?? completed
                    }
                    for call in pendingCalls {
                        _ = try? await chats.appendTurn(
                            TurnDraft(
                                role: .tool,
                                content: "[cancelled before this tool ran]",
                                stats: nil,
                                toolCallID: call.id,
                                toolName: call.name),
                            to: sessionID)
                    }
                    continuation.finish(throwing: error)
                }
                gate.end(sessionID)
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    static func truncatedToolResult(_ result: String) -> String {
        let clip = TextBudget.clip(result, to: toolResultContextBudgetBytes)
        guard clip.overflowed else { return result }
        return "[truncated: showing first \(toolResultContextBudgetBytes) of \(clip.total) bytes]\n"
            + clip.kept
    }

    static func messages(
        from turns: [ChatTurn],
        attachmentLoader: (@Sendable ([String]) -> [(ref: String, attachment: ChatAttachment)])? =
            nil
    ) -> [ChatMessage] {
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
            let calls = turn.toolCalls
            let pairs =
                turn.attachmentRefs.isEmpty ? [] : (attachmentLoader?(turn.attachmentRefs) ?? [])
            let attachments = pairs.map(\.attachment)
            if turn.role == .tool {
                messages.append(
                    ChatMessage(
                        role: .tool, content: turn.content,
                        toolCallID: turn.toolCallID, toolName: turn.toolName))
            } else if let role = turn.role.messageRole, !calls.isEmpty {
                messages.append(
                    ChatMessage(role: role, content: turn.content, toolCalls: calls))
            } else if let role = turn.role.messageRole,
                !turn.content.isEmpty || !turn.attachmentRefs.isEmpty
            {
                var content =
                    turn.role == .assistant && turn.interrupted
                    ? turn.content + Self.interruptedMarker : turn.content
                let blocks = attachments.compactMap(\.inlineBlock)
                if !blocks.isEmpty {
                    content = (blocks + [content]).filter { !$0.isEmpty }
                        .joined(separator: "\n\n")
                }
                let imagePairs = pairs.filter { $0.attachment.kind == .image }
                let images = imagePairs.map(\.attachment)
                let imageRefs = imagePairs.map(\.ref)
                if let last = messages.last, last.role == role, last.toolCalls.isEmpty,
                    last.toolCallID == nil
                {
                    messages[messages.count - 1] = ChatMessage(
                        role: role, content: last.content + Self.mergeBoundary + content,
                        attachments: last.attachments + images,
                        attachmentRefs: last.attachmentRefs + imageRefs)
                } else {
                    messages.append(
                        ChatMessage(
                            role: role, content: content, attachments: images,
                            attachmentRefs: imageRefs))
                }
            }
            index += 1
        }
        return messages
    }

    private func projected(_ turns: [ChatTurn]) -> [ChatMessage] {
        guard let store = attachments else { return Self.messages(from: turns) }
        let loader: @Sendable ([String]) -> [(ref: String, attachment: ChatAttachment)] = {
            store.loadPairs($0)
        }
        return Self.messages(from: turns, attachmentLoader: loader)
    }
}
