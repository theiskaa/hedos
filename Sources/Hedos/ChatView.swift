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
            _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
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
    let onOpenArtifacts: (() -> Void)?
    @State private var model: ChatViewModel
    @State private var followsStream = true
    @State private var expandedThinking: Set<String> = []
    @State private var expandedVersions: Set<String> = []
    @State private var showModelPicker = false
    @State private var editingEntryID: String?
    @State private var editText = ""

    init(
        session: ChatSession, library: LibraryViewModel, kernel: Kernel,
        onSessionsChanged: (() -> Void)? = nil,
        onOpenArtifacts: (() -> Void)? = nil
    ) {
        self.session = session
        self.library = library
        self.onOpenArtifacts = onOpenArtifacts
        let viewModel = ChatViewModel(kernel: kernel, session: session)
        viewModel.onSessionsChanged = onSessionsChanged
        _model = State(initialValue: viewModel)
    }

    private var boundRecord: ModelRecord? {
        library.record(id: model.boundModelID)
    }

    var body: some View {
        VStack(spacing: 0) {
            transcript
            if let notice = model.notice {
                noticeBar(notice)
            }
            composer
        }
        .navigationTitle(session.title)
        .navigationSubtitle(boundRecord?.runtime.id ?? "")
        .task(id: session.id) { await model.load() }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20) {
                    if model.transcript.isEmpty {
                        emptyTranscript
                    }
                    ForEach(model.transcript) { entry in
                        turn(entry)
                    }
                    Color.clear.frame(height: 1).id("tail")
                }
                .padding(.horizontal, 28)
                .padding(.vertical, 20)
                .frame(maxWidth: 720, alignment: .leading)
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
                    Text(entry.text)
                        .font(.system(size: 13))
                        .textSelection(.enabled)
                        .padding(.horizontal, 13)
                        .padding(.vertical, 8)
                        .background(
                            .quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 14))
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
            .frame(maxWidth: 420, alignment: .trailing)
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
                        .foregroundStyle(.tertiary)
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
                .font(.system(size: 13))
                .lineLimit(1...8)
                .padding(.horizontal, 13)
                .padding(.vertical, 8)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 14))
                .overlay(
                    RoundedRectangle(cornerRadius: 14)
                        .strokeBorder(.tertiary, lineWidth: 1))
            HStack(spacing: 8) {
                Button("Cancel") { editingEntryID = nil }
                    .controlSize(.small)
                Button("Send") {
                    editingEntryID = nil
                    model.edit(entry, text: editText)
                }
                .controlSize(.small)
                .buttonStyle(.borderedProminent)
                .disabled(
                    editText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    @ViewBuilder
    private func previousVersionsAffordance(_ entry: ChatViewModel.Entry) -> some View {
        if let versions = model.previousVersions[entry.id], !versions.isEmpty {
            VStack(
                alignment: entry.role == .user ? .trailing : .leading, spacing: 6
            ) {
                Button {
                    if expandedVersions.contains(entry.id) {
                        expandedVersions.remove(entry.id)
                    } else {
                        expandedVersions.insert(entry.id)
                    }
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "clock.arrow.circlepath")
                            .font(.system(size: 8))
                        Text(
                            versions.count == 1
                                ? "Previous version" : "\(versions.count) previous versions")
                        Image(
                            systemName: expandedVersions.contains(entry.id)
                                ? "chevron.down" : "chevron.right")
                        .font(.system(size: 7, weight: .semibold))
                    }
                    .font(.system(size: 10))
                    .foregroundStyle(.quaternary)
                }
                .buttonStyle(.plain)
                if expandedVersions.contains(entry.id) {
                    ForEach(versions) { version in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(version.role.rawValue.capitalized)
                                .font(.system(size: 9, weight: .medium))
                                .foregroundStyle(.quaternary)
                            Text(version.content)
                                .font(.system(size: 12))
                                .foregroundStyle(.tertiary)
                                .textSelection(.enabled)
                        }
                        .padding(.leading, 10)
                        .overlay(alignment: .leading) {
                            Rectangle()
                                .fill(.quaternary)
                                .frame(width: 2)
                        }
                    }
                }
            }
        }
    }

    private func artifactCard(_ reference: String) -> some View {
        Button {
            onOpenArtifacts?()
        } label: {
            HStack(spacing: 7) {
                Image(systemName: "photo.stack")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                Text("Generated artifact")
                    .font(.system(size: 12))
                    .foregroundStyle(.secondary)
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 9))
        }
        .buttonStyle(.plain)
        .help("Open in the gallery")
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @ViewBuilder
    private func thinkingBlock(_ entry: ChatViewModel.Entry) -> some View {
        let streaming = entry.text.isEmpty && model.isStreaming
        VStack(alignment: .leading, spacing: 6) {
            Button {
                if expandedThinking.contains(entry.id) {
                    expandedThinking.remove(entry.id)
                } else {
                    expandedThinking.insert(entry.id)
                }
            } label: {
                HStack(spacing: 5) {
                    Text(streaming ? "Thinking…" : "Thought")
                    Image(
                        systemName: expandedThinking.contains(entry.id)
                            ? "chevron.down" : "chevron.right")
                    .font(.system(size: 8, weight: .semibold))
                }
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
            if expandedThinking.contains(entry.id) {
                Text(entry.thinking)
                    .font(.system(size: 11.5))
                    .lineSpacing(3)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .padding(.leading, 10)
                    .overlay(alignment: .leading) {
                        Rectangle()
                            .fill(.quaternary)
                            .frame(width: 2)
                    }
            }
        }
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
            .foregroundStyle(.quaternary)
    }

    private var emptyTranscript: some View {
        Text(
            boundRecord.map { "Chatting with \($0.name), locally." }
                ?? "Pick a model to start this conversation."
        )
        .font(.system(size: 12))
        .foregroundStyle(.tertiary)
        .frame(maxWidth: .infinity)
        .padding(.top, 100)
    }

    private func noticeBar(_ notice: String) -> some View {
        HStack(spacing: 10) {
            Text(notice)
                .font(.system(size: 12))
                .foregroundStyle(Design.warn)
            if model.canStartOllama {
                Button("Start Ollama") {
                    model.startOllamaAndRetry()
                }
                .controlSize(.small)
            }
            Spacer()
        }
        .padding(.horizontal, 28)
        .padding(.bottom, 6)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.transcriptCharacterCount > 16000 {
                Text("This conversation is getting long — early turns may drop out of context.")
                    .font(.system(size: 10))
                    .foregroundStyle(.quaternary)
                    .padding(.leading, 6)
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextField(placeholder, text: $model.draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .lineLimit(1...8)
                    .onSubmit { model.send() }
                    .onKeyPress(.return, phases: .down) { press in
                        guard press.modifiers.contains(.shift) else { return .ignored }
                        model.draft += "\n"
                        return .handled
                    }
                    .padding(.leading, 6)
                    .padding(.vertical, 3)
                if model.isStreaming {
                    Button {
                        model.stop()
                    } label: {
                        Image(systemName: "stop.fill")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(.white)
                            .frame(width: 26, height: 26)
                            .background(Design.warn, in: Circle())
                    }
                    .buttonStyle(.plain)
                    .help("Stop generating")
                } else {
                    Button {
                        model.send()
                    } label: {
                        Image(systemName: "arrow.up")
                            .font(.system(size: 12, weight: .bold))
                            .foregroundStyle(sendable ? .white : .secondary)
                            .frame(width: 26, height: 26)
                            .background(
                                sendable
                                    ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.quaternary),
                                in: Circle())
                    }
                    .buttonStyle(.plain)
                    .disabled(!sendable)
                    .help("Send")
                }
            }
            modelChip
                .padding(.leading, 4)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 7)
        .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 22))
        .overlay(
            RoundedRectangle(cornerRadius: 22)
                .strokeBorder(.quaternary, lineWidth: 1))
        .padding(.horizontal, 20)
        .padding(.bottom, 16)
        .padding(.top, 8)
    }

    private var modelChip: some View {
        Button {
            showModelPicker = true
        } label: {
            HStack(spacing: 4) {
                Text(boundRecord?.name ?? "Choose model")
                    .lineLimit(1)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 8, weight: .semibold))
            }
            .font(.system(size: 11))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(.quaternary.opacity(0.5), in: Capsule())
        }
        .buttonStyle(.plain)
        .help("Switch the model behind this chat")
        .disabled(model.isStreaming)
        .popover(isPresented: $showModelPicker, arrowEdge: .top) {
            ChatModelPicker(
                library: library,
                boundID: model.boundModelID,
                defaultID: model.defaultModelID
            ) { record in
                showModelPicker = false
                model.rebind(to: record)
            } onMakeDefault: { record in
                model.makeDefault(record)
            }
        }
    }

    private var placeholder: String {
        boundRecord.map { "Message \($0.name)…" } ?? "Pick a model first…"
    }

    private var sendable: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && model.boundModelID != nil
    }
}

struct ChatModelPicker: View {
    let library: LibraryViewModel
    let boundID: String?
    let defaultID: String?
    let onPick: (ModelRecord) -> Void
    let onMakeDefault: (ModelRecord) -> Void

    private var groups: [(section: String, records: [ModelRecord])] {
        LibraryViewModel.grouped(
            library.records.filter {
                $0.state == .ready && Launcher.destination(for: $0) == .chat
            })
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                if groups.isEmpty {
                    Text("No chat-capable model is ready.")
                        .font(.system(size: 12))
                        .foregroundStyle(.tertiary)
                        .padding(6)
                }
                ForEach(groups, id: \.section) { group in
                    Text(group.section.uppercased())
                        .font(.system(size: 10, weight: .medium))
                        .tracking(0.8)
                        .foregroundStyle(.tertiary)
                        .padding(.horizontal, 6)
                        .padding(.top, 8)
                    ForEach(group.records) { record in
                        row(record)
                    }
                }
            }
            .padding(8)
        }
        .frame(width: 280)
        .frame(maxHeight: 340)
    }

    private func row(_ record: ModelRecord) -> some View {
        Button {
            onPick(record)
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "checkmark")
                    .font(.system(size: 9, weight: .semibold))
                    .opacity(record.id == boundID ? 1 : 0)
                Text(record.name)
                    .font(.system(size: 12))
                    .lineLimit(1)
                Spacer(minLength: 8)
                if record.id == defaultID {
                    Text("default")
                        .font(Design.data(9))
                        .foregroundStyle(.tertiary)
                }
                Text(record.runtime.tier == .native ? "native" : "managed")
                    .font(Design.data(9))
                    .foregroundStyle(.quaternary)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .contextMenu {
            Button("Make Default") {
                onMakeDefault(record)
            }
        }
    }
}
