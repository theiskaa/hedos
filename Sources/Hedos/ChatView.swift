import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ChatViewModel {
    struct Entry: Identifiable, Hashable {
        var id: String = UUID().uuidString
        var role: TurnRole
        var text: String
        var thinking: String = ""
        var stats: GenerationStats?
        var artifactRefs: [String] = []
        var persisted = false
    }

    private let kernel: Kernel
    let sessionID: String
    private var streamTask: Task<Void, Never>?

    var transcript: [Entry] = []
    var previousVersions: [String: [ChatTurn]] = [:]
    var draft = ""
    var isStreaming = false
    var notice: String?
    var canStartOllama = false
    var boundModelID: String?
    var defaultModelID: String?
    var onSessionsChanged: (() -> Void)?

    init(kernel: Kernel, session: ChatSession) {
        self.kernel = kernel
        self.sessionID = session.id
        self.boundModelID = session.modelID
    }

    func load() async {
        if let stored = try? await kernel.chats.session(id: sessionID) {
            apply(stored)
        }
        defaultModelID = (try? await kernel.defaultChatModelID()) ?? nil
    }

    private func apply(_ stored: ChatTranscript) {
        boundModelID = stored.session.modelID
        var previous: [String: [ChatTurn]] = [:]
        for turn in stored.turns {
            if let supersededBy = turn.supersededBy {
                previous[supersededBy, default: []].append(turn)
            }
        }
        previousVersions = previous
        transcript = stored.turns
            .filter { $0.supersededBy == nil && $0.role != .system }
            .map {
                Entry(
                    id: $0.id,
                    role: $0.role,
                    text: $0.content,
                    thinking: $0.thinking ?? "",
                    stats: $0.stats,
                    artifactRefs: $0.artifactRefs,
                    persisted: true)
            }
    }

    var transcriptCharacterCount: Int {
        transcript.reduce(0) { $0 + $1.text.count }
    }

    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isStreaming, boundModelID != nil else { return }
        draft = ""
        transcript.append(Entry(role: .user, text: text))
        stream { kernel, sessionID in
            try await kernel.sendChat(sessionID: sessionID, text: text)
        }
    }

    func rebind(to record: ModelRecord) {
        guard record.id != boundModelID else { return }
        boundModelID = record.id
        let kernel = kernel
        let sessionID = sessionID
        Task {
            try? await kernel.chats.rebindSession(id: sessionID, modelID: record.id)
            onSessionsChanged?()
        }
    }

    func makeDefault(_ record: ModelRecord) {
        defaultModelID = record.id
        let kernel = kernel
        Task {
            try? await kernel.setDefaultChatModel(record.id)
        }
    }

    func edit(_ entry: Entry, text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !isStreaming, entry.role == .user, entry.persisted, !trimmed.isEmpty else { return }
        guard let index = transcript.firstIndex(where: { $0.id == entry.id }) else { return }
        transcript.removeSubrange(index...)
        transcript.append(Entry(role: .user, text: trimmed))
        stream { kernel, sessionID in
            try await kernel.editChatTurn(sessionID: sessionID, turnID: entry.id, text: trimmed)
        }
    }

    func regenerate(_ entry: Entry) {
        guard !isStreaming, entry.role == .assistant, entry.persisted else { return }
        guard let index = transcript.firstIndex(where: { $0.id == entry.id }) else { return }
        transcript.removeSubrange(index...)
        stream { kernel, sessionID in
            try await kernel.regenerateChatTurn(sessionID: sessionID, turnID: entry.id)
        }
    }

    func startOllamaAndRetry() {
        guard !isStreaming else { return }
        canStartOllama = false
        notice = "Starting Ollama…"
        Task {
            do {
                try await kernel.startOllama()
                notice = nil
                if transcript.last?.role == .user {
                    stream { kernel, sessionID in
                        try await kernel.continueChat(sessionID: sessionID)
                    }
                }
            } catch {
                notice = error.localizedDescription
            }
        }
    }

    func stop() {
        streamTask?.cancel()
        isStreaming = false
    }

    private func stream(
        _ start: @escaping @Sendable (Kernel, String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >
    ) {
        notice = nil
        canStartOllama = false
        transcript.append(Entry(role: .assistant, text: ""))
        isStreaming = true

        streamTask = Task { [weak self] in
            guard let self else { return }
            var pendingText = ""
            var pendingThinking = ""
            let clock = ContinuousClock()
            var lastFlush = clock.now

            @MainActor func flush() {
                guard !transcript.isEmpty else { return }
                if !pendingText.isEmpty {
                    transcript[transcript.count - 1].text += pendingText
                    pendingText = ""
                }
                if !pendingThinking.isEmpty {
                    transcript[transcript.count - 1].thinking += pendingThinking
                    pendingThinking = ""
                }
            }

            do {
                let stream = try await start(kernel, sessionID)
                for try await chunk in stream {
                    switch chunk {
                    case .text(let delta):
                        pendingText += delta
                    case .thinking(let delta):
                        pendingThinking += delta
                    case .done(let stats):
                        if !transcript.isEmpty {
                            transcript[transcript.count - 1].stats = stats
                        }
                    default:
                        break
                    }
                    if clock.now - lastFlush > .milliseconds(50) {
                        flush()
                        lastFlush = clock.now
                    }
                }
                flush()
            } catch KernelError.runtimeUnavailable(let hint) {
                flush()
                notice = hint
                canStartOllama = hint.contains("ollama serve")
                dropEmptyAssistantTail()
            } catch is CancellationError {
                flush()
            } catch {
                flush()
                notice = error.localizedDescription
                dropEmptyAssistantTail()
            }
            isStreaming = false
            guard !Task.isCancelled else { return }
            _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
            guard !Task.isCancelled else { return }
            if let stored = try? await kernel.chats.session(id: sessionID) {
                apply(stored)
            }
            onSessionsChanged?()
        }
    }

    private func dropEmptyAssistantTail() {
        if let last = transcript.last, last.role == .assistant, last.text.isEmpty {
            transcript.removeLast()
        }
    }
}

struct ChatView: View {
    let session: ChatSession
    let library: LibraryViewModel
    let kernel: Kernel
    let onOpenArtifacts: ((String) -> Void)?
    @State private var model: ChatViewModel
    @State private var followsStream = true
    @State private var expandedThinking: Set<String> = []
    @State private var expandedVersions: Set<String> = []
    @State private var editingEntryID: String?
    @State private var editText = ""

    init(
        session: ChatSession, library: LibraryViewModel, kernel: Kernel,
        onSessionsChanged: (() -> Void)? = nil,
        onOpenArtifacts: ((String) -> Void)? = nil
    ) {
        self.session = session
        self.library = library
        self.kernel = kernel
        self.onOpenArtifacts = onOpenArtifacts
        let viewModel = ChatViewModel(kernel: kernel, session: session)
        viewModel.onSessionsChanged = onSessionsChanged
        _model = State(initialValue: viewModel)
    }

    private var boundRecord: ModelRecord? {
        library.record(id: model.boundModelID)
    }

    var body: some View {
        ConversationScaffold(
            placeholder: placeholder,
            draft: $model.draft,
            isWorking: model.isStreaming,
            canSend: sendable,
            notice: contextNotice ?? model.notice,
            noticeActionLabel: model.canStartOllama ? "Start Ollama" : nil,
            noticeAction: model.canStartOllama ? { model.startOllamaAndRetry() } : nil,
            onSend: { model.send() },
            onStop: { model.stop() },
            transcript: { transcript },
            aux: {},
            chip: { modelChip }
        )
        .task(id: session.id) { await model.load() }
    }

    private var contextNotice: String? {
        guard model.transcriptCharacterCount > 16000 else { return nil }
        return "This conversation is getting long — early turns may drop out of context."
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Design.Space.xxl) {
                    if model.transcript.isEmpty {
                        emptyTranscript
                    }
                    ForEach(model.transcript) { entry in
                        turn(entry)
                    }
                    Color.clear.frame(height: 1).id("tail")
                }
                .padding(.horizontal, Design.Space.xxl)
                .padding(.vertical, Design.Space.xxl)
                .frame(maxWidth: Design.conversationMaxWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentOffset.y + geometry.containerSize.height
                    >= geometry.contentSize.height - 60
            } action: { _, nearBottom in
                followsStream = nearBottom
            }
            .onChange(of: model.transcript) {
                if followsStream {
                    proxy.scrollTo("tail", anchor: .bottom)
                }
            }
        }
    }

    @ViewBuilder
    private func turn(_ entry: ChatViewModel.Entry) -> some View {
        if entry.role == .user {
            VStack(alignment: .trailing, spacing: 4) {
                if editingEntryID == entry.id {
                    editField(entry)
                } else {
                    PromptBubble(text: entry.text)
                        .contextMenu {
                            Button("Copy") { copy(entry.text) }
                            if entry.persisted && !model.isStreaming {
                                Button("Edit…") {
                                    editText = entry.text
                                    editingEntryID = entry.id
                                }
                            }
                        }
                }
                previousVersionsAffordance(entry)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            VStack(alignment: .leading, spacing: 8) {
                if !entry.thinking.isEmpty {
                    thinkingBlock(entry)
                }
                if !entry.text.isEmpty {
                    MarkdownTurnView(text: entry.text)
                        .contextMenu {
                            Button("Copy") { copy(entry.text) }
                            if entry.persisted && !model.isStreaming {
                                Button("Regenerate") { model.regenerate(entry) }
                            }
                        }
                } else if model.isStreaming && entry.thinking.isEmpty {
                    Text("…")
                        .foregroundStyle(Design.inkFaint)
                }
                ForEach(entry.artifactRefs, id: \.self) { reference in
                    artifactCard(reference)
                }
                if let stats = entry.stats, !entry.text.isEmpty {
                    statsLine(stats)
                }
                previousVersionsAffordance(entry)
            }
            .frame(maxWidth: 620, alignment: .leading)
        }
    }

    private func editField(_ entry: ChatViewModel.Entry) -> some View {
        VStack(alignment: .trailing, spacing: 6) {
            TextField("Edit message", text: $editText, axis: .vertical)
                .textFieldStyle(.plain)
                .font(Design.body)
                .lineLimit(1...8)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.m)
                .background(Design.bubbleFill, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
                .overlay(
                    RoundedRectangle(cornerRadius: Design.Radius.bubble)
                        .strokeBorder(Design.hairline, lineWidth: Design.hairlineWidth))
            HStack(spacing: 8) {
                Button("Cancel") { editingEntryID = nil }
                    .controlSize(.small)
                    .buttonStyle(.bordered)
                Button("Send") {
                    editingEntryID = nil
                    model.edit(entry, text: editText)
                }
                .controlSize(.small)
                .buttonStyle(.bordered)
                .disabled(
                    editText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    @ViewBuilder
    private func previousVersionsAffordance(_ entry: ChatViewModel.Entry) -> some View {
        if let versions = model.previousVersions[entry.id], !versions.isEmpty {
            DisclosureGroup(
                isExpanded: Binding(
                    get: { expandedVersions.contains(entry.id) },
                    set: { expanded in
                        if expanded {
                            expandedVersions.insert(entry.id)
                        } else {
                            expandedVersions.remove(entry.id)
                        }
                    })
            ) {
                VStack(alignment: .leading, spacing: Design.Space.s) {
                    ForEach(versions) { version in
                        VStack(alignment: .leading, spacing: Design.Space.xxs) {
                            Text(version.role.rawValue.uppercased())
                                .font(Design.micro)
                                .tracking(Design.microTracking)
                                .foregroundStyle(Design.inkFaint)
                            Text(version.content)
                                .font(Design.caption)
                                .foregroundStyle(Design.inkFaint)
                                .textSelection(.enabled)
                        }
                        .leftRule()
                    }
                }
                .padding(.top, Design.Space.xs)
            } label: {
                HStack(spacing: Design.Space.xs) {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(Design.glyphSmall)
                    Text(
                        versions.count == 1
                            ? "Previous version" : "\(versions.count) previous versions")
                }
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
            }
            .disclosureGroupStyle(QuietDisclosureStyle())
            .accessibilityLabel("Previous versions")
        }
    }

    private func artifactCard(_ reference: String) -> some View {
        ArtifactExchangeView(reference: reference, kernel: kernel)
            .contextMenu {
                Button("Show in Images") {
                    onOpenArtifacts?(reference)
                }
            }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @ViewBuilder
    private func thinkingBlock(_ entry: ChatViewModel.Entry) -> some View {
        let streaming = entry.text.isEmpty && model.isStreaming
        return DisclosureGroup(
            isExpanded: Binding(
                get: { expandedThinking.contains(entry.id) },
                set: { expanded in
                    if expanded {
                        expandedThinking.insert(entry.id)
                    } else {
                        expandedThinking.remove(entry.id)
                    }
                })
        ) {
            Text(entry.thinking)
                .font(Design.label)
                .lineSpacing(Design.bodyLineSpacing)
                .foregroundStyle(Design.inkSoft)
                .textSelection(.enabled)
                .leftRule()
                .padding(.top, Design.Space.xs)
        } label: {
            Text(streaming ? "Thinking…" : "Thought")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
        }
        .disclosureGroupStyle(QuietDisclosureStyle())
        .accessibilityLabel(streaming ? "Model thinking" : "Model thoughts")
    }

    private func statsLine(_ stats: GenerationStats) -> some View {
        var parts: [String] = []
        if let ttft = stats.ttftMs {
            parts.append(String(format: "ttft %.1fs", Double(ttft) / 1000))
        }
        if let tokens = stats.completionTokens {
            let generationMs = (stats.durationMs ?? 0) - (stats.ttftMs ?? 0)
            if generationMs > 0 {
                parts.append(
                    String(format: "%.0f tok/s", Double(tokens) / Double(generationMs) * 1000))
            } else if let ms = stats.durationMs, ms > 0 {
                parts.append(String(format: "%.0f tok/s", Double(tokens) / Double(ms) * 1000))
            }
            parts.append("\(tokens) tok")
        }
        return Text(parts.joined(separator: " · "))
            .font(Design.data(10))
            .foregroundStyle(Design.inkFaint.opacity(0.7))
    }

    private var emptyTranscript: some View {
        Text(
            boundRecord.map { "Chatting with \($0.name), locally." }
                ?? "Pick a model to start this conversation."
        )
        .font(Design.caption)
        .foregroundStyle(Design.inkFaint)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var modelChip: some View {
        ChipMenu(title: boundRecord?.name ?? "Choose model") {
            if chatGroups.isEmpty {
                Text("No chat-capable model is ready.")
            }
            ForEach(chatGroups, id: \.section) { group in
                Section(group.section) {
                    ForEach(group.records) { record in
                        Button {
                            model.rebind(to: record)
                        } label: {
                            if record.id == model.boundModelID {
                                Label(menuTitle(record), systemImage: "checkmark")
                            } else {
                                Text(menuTitle(record))
                            }
                        }
                    }
                }
            }
            if let bound = boundRecord, bound.id != model.defaultModelID {
                Divider()
                Button("Make \(bound.name) the Default") {
                    model.makeDefault(bound)
                }
            }
        }
        .disabled(model.isStreaming)
        .accessibilityLabel("Chat model")
    }

    private var chatGroups: [(section: String, records: [ModelRecord])] {
        LibraryViewModel.grouped(
            library.records.filter {
                $0.state == .ready && Launcher.destination(for: $0) == .chat
            })
    }

    private func menuTitle(_ record: ModelRecord) -> String {
        var parts = [record.name]
        parts.append(record.runtime.tier == .native ? "native" : "managed")
        if record.id == model.defaultModelID {
            parts.append("default")
        }
        return parts.joined(separator: " · ")
    }

    private var placeholder: String {
        boundRecord.map { "Message \($0.name)…" } ?? "Pick a model first…"
    }

    private var sendable: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && model.boundModelID != nil
    }
}

