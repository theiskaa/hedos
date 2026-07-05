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

    init(record: ModelRecord, kernel: Kernel) {
        self.record = record
        _model = State(initialValue: ChatViewModel(kernel: kernel, modelID: record.id))
    }

    var body: some View {
        VStack(spacing: 0) {
            transcript
            Divider()
            if let notice = model.notice {
                HStack(spacing: 12) {
                    Label(notice, systemImage: "exclamationmark.triangle")
                        .font(.callout)
                        .foregroundStyle(Design.terracotta)
                    if model.canStartOllama {
                        Button("Start Ollama") {
                            model.startOllamaAndRetry()
                        }
                        .controlSize(.small)
                    }
                    Spacer()
                }
                .padding(.horizontal, 18)
                .padding(.top, 8)
            }
            composer
        }
        .navigationTitle(record.name)
        .navigationSubtitle(record.runtime.id ?? "")
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    if model.transcript.isEmpty {
                        emptyTranscript
                    }
                    ForEach(model.transcript) { entry in
                        MessageBubble(entry: entry, isStreaming: model.isStreaming)
                            .frame(
                                maxWidth: .infinity,
                                alignment: entry.role == .user ? .trailing : .leading)
                    }
                    Color.clear.frame(height: 1).id("tail")
                }
                .padding(18)
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

    private var emptyTranscript: some View {
        VStack(spacing: 8) {
            Image(systemName: Design.modalityGlyph(record.modality))
                .font(.system(size: 26))
                .foregroundStyle(Design.modalityColor(record.modality).opacity(0.6))
            Text("Chatting with \(record.name), locally.")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 80)
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField("Message \(record.name)…", text: $model.draft, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...6)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
                .onSubmit { model.send() }
            if model.isStreaming {
                Button {
                    model.stop()
                } label: {
                    Image(systemName: "stop.circle.fill")
                        .font(.system(size: 24))
                        .foregroundStyle(Design.terracotta)
                }
                .buttonStyle(.plain)
                .help("Stop generating")
            } else {
                Button {
                    model.send()
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 24))
                        .foregroundStyle(
                            model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                ? AnyShapeStyle(.tertiary) : AnyShapeStyle(Design.accent))
                }
                .buttonStyle(.plain)
                .disabled(model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .help("Send")
            }
        }
        .padding(14)
    }
}

struct MessageBubble: View {
    let entry: ChatViewModel.Entry
    let isStreaming: Bool

    var body: some View {
        VStack(alignment: entry.role == .user ? .trailing : .leading, spacing: 5) {
            bubble
            if entry.role == .assistant, let stats = entry.stats, !entry.text.isEmpty {
                statsLine(stats)
            }
        }
        .frame(maxWidth: 520, alignment: entry.role == .user ? .trailing : .leading)
    }

    @ViewBuilder
    private var bubble: some View {
        let isUser = entry.role == .user
        VStack(alignment: .leading, spacing: 7) {
            if !entry.thinking.isEmpty {
                DisclosureGroup {
                    Text(entry.thinking)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 4)
                } label: {
                    Label(
                        entry.text.isEmpty && isStreaming ? "Thinking…" : "Thought",
                        systemImage: "brain")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(Design.lapis)
                }
            }
            if !entry.text.isEmpty || entry.thinking.isEmpty {
                Text(entry.text.isEmpty && isStreaming ? "…" : entry.text)
                    .textSelection(.enabled)
                    .lineSpacing(2)
            }
        }
        .padding(.horizontal, 13)
        .padding(.vertical, 9)
        .background(
            isUser
                ? AnyShapeStyle(Design.accent.opacity(0.16))
                : AnyShapeStyle(.quaternary.opacity(0.7)),
            in: RoundedRectangle(cornerRadius: 11))
    }

    private func statsLine(_ stats: GenerationStats) -> some View {
        var parts: [String] = []
        if let tokens = stats.completionTokens {
            parts.append("\(tokens) tokens")
            if let ms = stats.durationMs, ms > 0 {
                parts.append(String(format: "%.0f tok/s", Double(tokens) / Double(ms) * 1000))
            }
        }
        return Text(parts.joined(separator: " · "))
            .font(Design.data(10))
            .foregroundStyle(.tertiary)
            .padding(.horizontal, 4)
    }
}
