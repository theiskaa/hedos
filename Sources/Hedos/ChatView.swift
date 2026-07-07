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
    private let readAloudPlayer = PCMPlayer()
    private var readAloudTask: Task<Void, Never>?

    var transcript: [Entry] = []
    var previousVersions: [String: [ChatTurn]] = [:]
    var draft = ""
    var isStreaming = false
    var notice: String?
    var canStartOllama = false
    var boundModelID: String?
    var defaultModelID: String?
    var speakingEntryID: String?
    var showsStreamCursor = false
    var streamStatus: String?
    var onSessionsChanged: (() -> Void)?
    var recordsProvider: (() -> [ModelRecord])?
    private var reveal = PacedReveal()
    private var lastDeltaAt = ContinuousClock().now
    private var tickerTask: Task<Void, Never>?

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
        stopTicker()
        isStreaming = false
    }

    private func tickReveal() {
        guard isStreaming else { return }
        if reveal.tick(), !transcript.isEmpty {
            transcript[transcript.count - 1].text = reveal.revealed
        }
        let quiet = ContinuousClock().now - lastDeltaAt > .milliseconds(150)
        let cursor = isStreaming && quiet && reveal.backlog == 0 && reveal.revealedCount > 0
        if showsStreamCursor != cursor {
            showsStreamCursor = cursor
        }
    }

    private func startTicker() {
        tickerTask?.cancel()
        tickerTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(16))
                guard let self else { return }
                self.tickReveal()
            }
        }
    }

    private func stopTicker() {
        tickerTask?.cancel()
        tickerTask = nil
        showsStreamCursor = false
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
        reveal.reset()
        lastDeltaAt = ContinuousClock().now
        startTicker()

        streamTask = Task { [weak self] in
            guard let self else { return }
            var pendingThinking = ""
            let clock = ContinuousClock()
            var lastThinkingFlush = clock.now

            @MainActor func flushThinking() {
                guard !transcript.isEmpty, !pendingThinking.isEmpty else { return }
                transcript[transcript.count - 1].thinking += pendingThinking
                pendingThinking = ""
            }

            @MainActor func settle() {
                flushThinking()
                guard !transcript.isEmpty else { return }
                reveal.finish()
                transcript[transcript.count - 1].text = reveal.revealed
            }

            do {
                let stream = try await start(kernel, sessionID)
                for try await chunk in stream {
                    switch chunk {
                    case .text(let delta):
                        reveal.append(delta)
                        lastDeltaAt = clock.now
                        if streamStatus != nil {
                            streamStatus = nil
                        }
                    case .thinking(let delta):
                        pendingThinking += delta
                        if streamStatus != nil {
                            streamStatus = nil
                        }
                    case .status(let message):
                        streamStatus = message
                    case .done(let stats):
                        if !transcript.isEmpty {
                            transcript[transcript.count - 1].stats = stats
                        }
                    default:
                        break
                    }
                    if clock.now - lastThinkingFlush > .milliseconds(50) {
                        flushThinking()
                        lastThinkingFlush = clock.now
                    }
                }
                settle()
                Haptics.completion()
            } catch KernelError.runtimeUnavailable(let hint) {
                settle()
                notice = hint
                canStartOllama = hint.contains("ollama serve")
                dropEmptyAssistantTail()
            } catch is CancellationError {
                settle()
            } catch {
                settle()
                notice = error.localizedDescription
                dropEmptyAssistantTail()
            }
            stopTicker()
            streamStatus = nil
            isStreaming = false
            guard !Task.isCancelled else { return }
            _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
            guard !Task.isCancelled else { return }
            if let stored = try? await kernel.chats.session(id: sessionID) {
                apply(stored)
            }
            onSessionsChanged?()
            autoSpeakIfWanted()
        }
    }

    private func dropEmptyAssistantTail() {
        if let last = transcript.last, last.role == .assistant, last.text.isEmpty {
            transcript.removeLast()
        }
    }

    func speaker(in records: [ModelRecord]) -> ModelRecord? {
        records.first { $0.state == .ready && Launcher.destination(for: $0) == .voice }
    }

    func toggleReadAloud(_ entry: Entry) {
        if speakingEntryID == entry.id {
            stopReadAloud()
            return
        }
        stopReadAloud()
        guard entry.role == .assistant,
            let speaker = speaker(in: recordsProvider?() ?? [])
        else { return }
        let text = SpeechText.speakable(entry.text)
        guard !text.isEmpty else { return }
        speakingEntryID = entry.id
        readAloudTask = Task { [weak self] in
            guard let self else { return }
            var pcm = Data()
            var sampleRate = 24000
            do {
                let voices = (try? await kernel.voices(speaker.id)) ?? []
                var chosen: String?
                if case .string(let configured)? = speaker.paramValues["voice"],
                    voices.contains(configured)
                {
                    chosen = configured
                } else if let fallback = await kernel.voiceSettings().defaultVoice,
                    voices.contains(fallback)
                {
                    chosen = fallback
                } else {
                    chosen = voices.first
                }
                guard let voice = chosen else {
                    notice = "\(speaker.displayName) offers no voices to read with."
                    speakingEntryID = nil
                    return
                }
                var payload: [String: JSONValue] = [
                    "text": .string(text),
                    "voice": .string(voice),
                ]
                if speaker.paramValues["speed"] == nil {
                    let speed = await kernel.voiceSettings().speed
                    if speed != 1.0 {
                        payload["speed"] = .double(speed)
                    }
                }
                let stream = try await kernel.invoke(
                    speaker.id, .speak, payload: .object(payload))
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        sampleRate = frame.sampleRate
                        pcm.append(frame.data)
                        readAloudPlayer.enqueue(frame)
                    }
                }
                if !pcm.isEmpty, entry.persisted, !Task.isCancelled {
                    if let artifact = try? await kernel.saveSpeech(
                        modelID: speaker.id, voice: voice, text: text,
                        sampleRate: sampleRate, pcm: pcm)
                    {
                        try? await kernel.attachSpokenArtifact(
                            sessionID: sessionID, turnID: entry.id, artifactID: artifact.id)
                        if let stored = try? await kernel.chats.session(id: sessionID) {
                            apply(stored)
                        }
                        onSessionsChanged?()
                    }
                }
            } catch is CancellationError {
            } catch {
                notice = error.localizedDescription
            }
            if speakingEntryID == entry.id {
                speakingEntryID = nil
            }
        }
    }

    func stopReadAloud() {
        readAloudTask?.cancel()
        readAloudPlayer.stop()
        speakingEntryID = nil
    }

    func autoSpeakIfWanted() {
        let kernel = kernel
        Task { [weak self] in
            guard await kernel.voiceSettings().autoSpeak else { return }
            guard let self, !isStreaming, speakingEntryID == nil,
                let last = transcript.last, last.role == .assistant, !last.text.isEmpty
            else { return }
            toggleReadAloud(last)
        }
    }
}

struct ChatView: View {
    let session: ChatSession
    let library: LibraryViewModel
    let kernel: Kernel
    let onOpenArtifacts: ((String) -> Void)?
    let onNewChat: (() -> Void)?
    let onNarrate: ((String, String) -> Void)?
    @Environment(\.chatShowsStats) private var showsStats
    @Environment(\.conversationWidth) private var conversationWidth
    @Environment(\.transcriptSpacing) private var transcriptSpacing
    @State private var model: ChatViewModel
    @State private var followsStream = true
    @State private var expandedThinking: Set<String> = []
    @State private var expandedVersions: Set<String> = []
    @State private var editingEntryID: String?
    @State private var editText = ""
    @State private var modelMenuOpen = false
    @State private var voiceConversation = VoiceConversationController()
    @State private var copiedEntryID: String?

    init(
        session: ChatSession, library: LibraryViewModel, kernel: Kernel,
        onSessionsChanged: (() -> Void)? = nil,
        onOpenArtifacts: ((String) -> Void)? = nil,
        onNewChat: (() -> Void)? = nil,
        onNarrate: ((String, String) -> Void)? = nil
    ) {
        self.session = session
        self.library = library
        self.kernel = kernel
        self.onOpenArtifacts = onOpenArtifacts
        self.onNewChat = onNewChat
        self.onNarrate = onNarrate
        let viewModel = ChatViewModel(kernel: kernel, session: session)
        viewModel.onSessionsChanged = onSessionsChanged
        viewModel.recordsProvider = { [weak library] in library?.records ?? [] }
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
            slash: SlashSetup(kernel: kernel, capability: .chat, commands: slashCommands),
            dictation: DictationSetup(
                kernel: kernel,
                records: { [weak library] in library?.records ?? [] }),
            transcript: { transcript },
            aux: { voiceLoopControl },
            chip: { modelChip }
        )
        .task(id: session.id) { await model.load() }
        .onDisappear {
            model.stopReadAloud()
            voiceConversation.stop()
        }
    }

    @ViewBuilder
    private var voiceLoopControl: some View {
        if VoiceConversationController.participants(in: library.records) != nil {
            if voiceConversation.active, let status = voiceConversation.status {
                ShimmerText(text: status.uppercased())
                    .truncationMode(.tail)
                    .frame(maxWidth: Design.Column.control, alignment: .trailing)
            }
            CircleControl(
                glyph: voiceConversation.active ? "waveform.slash" : "waveform",
                prominent: voiceConversation.active,
                label: voiceConversation.active
                    ? "End voice conversation" : "Start voice conversation"
            ) {
                voiceConversation.toggle(
                    sessionID: session.id, kernel: kernel, records: library.records
                ) { [weak model] in
                    Task { await model?.load() }
                }
            }
            .accessibilityIdentifier("voice-conversation")
        }
    }

    private var slashCommands: [SlashCommand] {
        var commands = [
            SlashCommand(
                id: "model", title: "model", subtitle: "Choose the chat model",
                glyph: "square.stack.3d.up"
            ) {
                modelMenuOpen = true
            }
        ]
        if let onNewChat {
            commands.append(
                SlashCommand(
                    id: "new", title: "new", subtitle: "Start a new chat", glyph: "plus.message",
                    perform: onNewChat))
        }
        return commands
    }

    private var contextNotice: String? {
        guard model.transcriptCharacterCount > 16000 else { return nil }
        return "This conversation is getting long; early turns may drop out of context."
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: transcriptSpacing) {
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
                .frame(maxWidth: conversationWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .onScrollGeometryChange(for: Bool.self) { geometry in
                geometry.contentOffset.y + geometry.containerSize.height
                    >= geometry.contentSize.height - 60
            } action: { _, nearBottom in
                followsStream = nearBottom
            }
            .onChange(of: model.transcript) { old, new in
                guard followsStream else { return }
                proxy.scrollTo("tail", anchor: .bottom)
                if old.isEmpty && !new.isEmpty {
                    settleAtTail(proxy)
                }
            }
        }
    }

    private func settleAtTail(_ proxy: ScrollViewProxy) {
        Task {
            for delay in [120, 400] {
                try? await Task.sleep(for: .milliseconds(delay))
                guard followsStream else { return }
                proxy.scrollTo("tail", anchor: .bottom)
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
                    MarkdownTurnView(text: displayText(entry), cursor: showsCursor(entry))
                        .contextMenu {
                            Button("Copy") { copy(entry.text) }
                            if canReadAloud(entry) {
                                Button(
                                    model.speakingEntryID == entry.id
                                        ? "Stop Reading" : "Narrate"
                                ) {
                                    narrate(entry)
                                }
                            }
                            if entry.persisted && !model.isStreaming {
                                Button("Regenerate") { model.regenerate(entry) }
                            }
                        }
                } else if model.isStreaming && entry.thinking.isEmpty {
                    ShimmerText(
                        text: (model.streamStatus ?? "Streaming…").uppercased(),
                        font: Design.micro)
                }
                ForEach(entry.artifactRefs, id: \.self) { reference in
                    artifactCard(reference)
                }
                if !entry.text.isEmpty && !(model.isStreaming && !entry.persisted) {
                    HStack(spacing: Design.Space.l) {
                        copyControl(entry)
                        if canReadAloud(entry) {
                            readAloudControl(entry)
                        }
                        if showsStats, let stats = entry.stats {
                            statsLine(stats)
                        }
                    }
                }
                previousVersionsAffordance(entry)
            }
            .frame(maxWidth: Design.Column.transcriptProse, alignment: .leading)
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
                    .buttonStyle(QuietButtonStyle())
                Button("Send") {
                    editingEntryID = nil
                    model.edit(entry, text: editText)
                }
                .buttonStyle(QuietButtonStyle())
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
            if streaming {
                ShimmerText(text: "Thinking…", font: Design.label, tracked: false)
            } else {
                Text("Thought")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
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
        return HStack(spacing: Design.Space.xs) {
            ForEach(parts, id: \.self) { part in
                TintChip(text: part)
            }
        }
    }

    private func displayText(_ entry: ChatViewModel.Entry) -> String {
        let isLive =
            model.isStreaming && !entry.persisted && entry.id == model.transcript.last?.id
        guard isLive else { return entry.text }
        return MarkdownBalancer.balanced(entry.text)
    }

    private func showsCursor(_ entry: ChatViewModel.Entry) -> Bool {
        model.isStreaming && !entry.persisted && entry.id == model.transcript.last?.id
            && model.showsStreamCursor
    }

    private func canReadAloud(_ entry: ChatViewModel.Entry) -> Bool {
        entry.role == .assistant && !entry.text.isEmpty && !model.isStreaming
            && model.speaker(in: library.records) != nil
    }

    private func readAloudControl(_ entry: ChatViewModel.Entry) -> some View {
        let speaking = model.speakingEntryID == entry.id
        return HStack(spacing: Design.Space.s) {
            Button {
                narrate(entry)
            } label: {
                Image(systemName: speaking ? "stop.fill" : "speaker.wave.2")
                    .font(Design.glyphInline)
                    .foregroundStyle(speaking ? Design.ink : Design.inkFaint)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(speaking ? "Stop reading" : "Narrate in Voice")
            .accessibilityLabel(speaking ? "Stop reading" : "Narrate in Voice")
            if speaking {
                SpeakingIndicator()
            }
        }
    }

    private func copyControl(_ entry: ChatViewModel.Entry) -> some View {
        Button {
            copy(entry.text)
            copiedEntryID = entry.id
            Task {
                try? await Task.sleep(for: .seconds(1.5))
                if copiedEntryID == entry.id {
                    copiedEntryID = nil
                }
            }
        } label: {
            Image(systemName: copiedEntryID == entry.id ? "checkmark" : "doc.on.doc")
                .font(Design.glyphInline)
                .foregroundStyle(copiedEntryID == entry.id ? Design.ink : Design.inkFaint)
                .contentTransition(.symbolEffect(.replace))
                .contentShape(Rectangle())
        }
        .buttonStyle(PressDipStyle())
        .help("Copy the reply")
        .accessibilityLabel(copiedEntryID == entry.id ? "Copied" : "Copy the reply")
    }

    private func narrate(_ entry: ChatViewModel.Entry) {
        if model.speakingEntryID == entry.id {
            model.toggleReadAloud(entry)
            return
        }
        if let onNarrate {
            onNarrate(SpeechText.speakable(entry.text), entry.id)
        } else {
            model.toggleReadAloud(entry)
        }
    }

    private var emptyTranscript: some View {
        TranscriptEmptyState(
            eyebrow: "Chat · Local",
            headline: boundRecord != nil ? "Say the first thing." : "Pick a model to begin.",
            caption: boundRecord.map {
                "\($0.displayName) is loaded and listening. Nothing you type leaves this Mac. Type / for saved prompts, or tap the mic to dictate."
            }
                ?? "Every ready chat model on this Mac lives in the chip below the composer.")
    }

    private var modelChip: some View {
        InkMenu(
            title: boundRecord?.displayName ?? "Choose model",
            accessibilityName: "Chat model",
            externalOpen: $modelMenuOpen
        ) {
            if chatGroups.isEmpty {
                InkMenuRow(title: "No chat-capable model is ready.", disabled: true) {}
            }
            ForEach(chatGroups, id: \.section) { group in
                InkMenuHeader(title: group.section)
                ForEach(group.records) { record in
                    InkMenuRow(
                        title: record.displayName,
                        annotation: menuAnnotation(record),
                        selected: record.id == model.boundModelID
                    ) {
                        model.rebind(to: record)
                    }
                }
            }
            if let bound = boundRecord, bound.id != model.defaultModelID {
                InkMenuDivider()
                InkMenuRow(title: "Make \(bound.displayName) the Default") {
                    model.makeDefault(bound)
                }
            }
        }
        .disabled(model.isStreaming)
    }

    private var chatGroups: [(section: String, records: [ModelRecord])] {
        LibraryViewModel.grouped(
            library.records.filter {
                $0.state == .ready && Launcher.destination(for: $0) == .chat
            })
    }

    private func menuAnnotation(_ record: ModelRecord) -> String? {
        var parts: [String] = []
        parts.append(record.runtime.tier == .native ? "native" : "managed")
        if let fit = Fit.short(record) {
            parts.append(fit)
        }
        if record.id == model.defaultModelID {
            parts.append("default")
        }
        return parts.joined(separator: " · ")
    }

    private var placeholder: String {
        boundRecord.map { "Message \($0.displayName)…" } ?? "Pick a model first…"
    }

    private var sendable: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && model.boundModelID != nil
    }
}

