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

    init(kernel: Kernel, modelID: String) {
        self.kernel = kernel
        self.modelID = modelID
    }

    func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isStreaming else { return }
        notice = nil
        draft = ""
        transcript.append(Entry(role: .user, text: text))

        let history = transcript.map { ChatMessage(role: $0.role, content: $0.text) }
        transcript.append(Entry(role: .assistant, text: ""))
        isStreaming = true

        streamTask = Task { [weak self] in
            guard let self else { return }
            do {
                let stream = try await kernel.chat(modelID, messages: history)
                for try await chunk in stream {
                    guard !transcript.isEmpty else { continue }
                    switch chunk {
                    case .text(let delta):
                        transcript[transcript.count - 1].text += delta
                    case .thinking(let delta):
                        transcript[transcript.count - 1].thinking += delta
                    case .done:
                        break
                    }
                }
            } catch KernelError.runtimeUnavailable(let hint) {
                notice = hint
                dropEmptyAssistantTail()
            } catch is CancellationError {
            } catch {
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
                Label(notice, systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
            }
            composer
        }
        .navigationTitle(record.name)
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    ForEach(model.transcript) { entry in
                        bubble(entry)
                            .frame(
                                maxWidth: .infinity,
                                alignment: entry.role == .user ? .trailing : .leading)
                    }
                    Color.clear.frame(height: 1).id("tail")
                }
                .padding(16)
            }
            .onChange(of: model.transcript) {
                proxy.scrollTo("tail", anchor: .bottom)
            }
        }
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
