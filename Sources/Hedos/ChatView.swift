import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ChatViewModel {
    struct Entry: Identifiable, Hashable {
        let id = UUID()
        var role: ChatMessage.Role
        var text: String
        var thinking: String = ""
        var stats: GenerationStats?
    }

    private let kernel: Kernel
    private let modelID: String
    private var streamTask: Task<Void, Never>?

    var transcript: [Entry] = []
    var draft = ""
    var isStreaming = false
    var notice: String?
    var canStartOllama = false

    init(kernel: Kernel, modelID: String) {
        self.kernel = kernel
        self.modelID = modelID
    }

    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isStreaming else { return }
        draft = ""
        transcript.append(Entry(role: .user, text: text))
        run()
    }

    func startOllamaAndRetry() {
        guard !isStreaming else { return }
        canStartOllama = false
        notice = "Starting Ollama…"
        Task {
            do {
                try await kernel.startOllama()
                notice = nil
                if transcript.last?.role == .user { run() }
            } catch {
                notice = error.localizedDescription
            }
        }
    }

    private func run() {
        notice = nil
        canStartOllama = false
        let history = transcript.map { ChatMessage(role: $0.role, content: $0.text) }
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
                let stream = try await kernel.chat(modelID, messages: history)
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
        }
    }

    func stop() {
        streamTask?.cancel()
        isStreaming = false
    }

    private func dropEmptyAssistantTail() {
        if let last = transcript.last, last.role == .assistant, last.text.isEmpty {
            transcript.removeLast()
        }
    }
}

struct ChatView: View {
    let record: ModelRecord
    @State private var model: ChatViewModel
    @State private var followsStream = true
    @State private var expandedThinking: Set<UUID> = []

    init(record: ModelRecord, kernel: Kernel) {
        self.record = record
        _model = State(initialValue: ChatViewModel(kernel: kernel, modelID: record.id))
    }

    var body: some View {
        VStack(spacing: 0) {
            transcript
            if let notice = model.notice {
                noticeBar(notice)
            }
            composer
        }
        .navigationTitle(record.name)
        .navigationSubtitle(record.runtime.id ?? "")
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
        Text("Chatting with \(record.name), locally.")
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
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message \(record.name)…", text: $model.draft, axis: .vertical)
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
                            sendable ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.quaternary),
                            in: Circle())
                }
                .buttonStyle(.plain)
                .disabled(!sendable)
                .help("Send")
            }
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

    private var sendable: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}
