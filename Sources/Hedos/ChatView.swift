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
            } catch KernelError.runtimeUnavailable(let hint) {
                notice = hint
            } catch {
                notice = "Could not start Ollama: \(error.localizedDescription)"
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
                notice = "Generation failed: \(error.localizedDescription)"
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
                        .foregroundStyle(.orange)
                    if model.canStartOllama {
                        Button("Start Ollama") {
                            model.startOllamaAndRetry()
                        }
                        .controlSize(.small)
                    }
                    Spacer()
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
            }
            composer
        }
        .navigationTitle(record.name)
    }

    private var transcript: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                ForEach(model.transcript) { entry in
                    bubble(entry)
                        .frame(
                            maxWidth: .infinity,
                            alignment: entry.role == .user ? .trailing : .leading)
                }
            }
            .padding(16)
        }
        .defaultScrollAnchor(.bottom)
    }

    @ViewBuilder
    private func bubble(_ entry: ChatViewModel.Entry) -> some View {
        let isUser = entry.role == .user
        VStack(alignment: .leading, spacing: 6) {
            if !entry.thinking.isEmpty {
                DisclosureGroup {
                    Text(entry.thinking)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } label: {
                    Label(
                        entry.text.isEmpty && model.isStreaming ? "Thinking…" : "Thought",
                        systemImage: "brain")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            if !entry.text.isEmpty || entry.thinking.isEmpty {
                Text(entry.text.isEmpty && model.isStreaming ? "…" : entry.text)
                    .textSelection(.enabled)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(
            isUser ? AnyShapeStyle(.tint.opacity(0.18)) : AnyShapeStyle(.quaternary),
            in: RoundedRectangle(cornerRadius: 10))
        .frame(maxWidth: 480, alignment: isUser ? .trailing : .leading)
    }

    private var composer: some View {
        HStack(spacing: 8) {
            TextField("Message \(record.name)…", text: $model.draft, axis: .vertical)
                .textFieldStyle(.plain)
                .lineLimit(1...5)
                .onSubmit { model.send() }
            if model.isStreaming {
                Button {
                    model.stop()
                } label: {
                    Image(systemName: "stop.circle.fill").imageScale(.large)
                }
                .buttonStyle(.plain)
                .help("Stop generating")
            } else {
                Button {
                    model.send()
                } label: {
                    Image(systemName: "arrow.up.circle.fill").imageScale(.large)
                }
                .buttonStyle(.plain)
                .disabled(model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .help("Send")
            }
        }
        .padding(12)
    }
}
