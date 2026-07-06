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
    }

    private let kernel: Kernel
    let sessionID: String
    private var streamTask: Task<Void, Never>?

    var transcript: [Entry] = []
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
            boundModelID = stored.session.modelID
            transcript = stored.turns
                .filter { $0.supersededBy == nil && $0.role != .system }
                .map {
                    Entry(
                        id: $0.id,
                        role: $0.role,
                        text: $0.content,
                        thinking: $0.thinking ?? "",
                        stats: $0.stats)
                }
        }
        defaultModelID = (try? await kernel.defaultChatModelID()) ?? nil
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
    @State private var model: ChatViewModel
    @State private var followsStream = true
    @State private var expandedThinking: Set<String> = []
    @State private var showModelPicker = false

    init(
        session: ChatSession, library: LibraryViewModel, kernel: Kernel,
        onSessionsChanged: (() -> Void)? = nil
    ) {
        self.session = session
        self.library = library
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
            Text(entry.text)
                .font(.system(size: 13))
                .textSelection(.enabled)
                .padding(.horizontal, 13)
                .padding(.vertical, 8)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 14))
                .frame(maxWidth: 420, alignment: .trailing)
                .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            VStack(alignment: .leading, spacing: 8) {
                if !entry.thinking.isEmpty {
                    thinkingBlock(entry)
                }
                if !entry.text.isEmpty {
                    Text(entry.text)
                        .font(.system(size: 13))
                        .lineSpacing(3.5)
                        .textSelection(.enabled)
                } else if model.isStreaming && entry.thinking.isEmpty {
                    Text("…")
                        .foregroundStyle(.tertiary)
                }
                if let stats = entry.stats, !entry.text.isEmpty {
                    statsLine(stats)
                }
            }
            .frame(maxWidth: 620, alignment: .leading)
        }
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
        if let tokens = stats.completionTokens {
            parts.append("\(tokens) tok")
            if let ms = stats.durationMs, ms > 0 {
                parts.append(String(format: "%.0f tok/s", Double(tokens) / Double(ms) * 1000))
            }
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
            HStack(alignment: .bottom, spacing: 8) {
                TextField(placeholder, text: $model.draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                    .lineLimit(1...8)
                    .onSubmit { model.send() }
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
