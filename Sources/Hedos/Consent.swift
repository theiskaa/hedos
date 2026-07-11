import HedosKernel
import SwiftUI

@MainActor
@Observable
final class ConsentCoordinator {
    static let shared = ConsentCoordinator()

    struct Pending: Identifiable {
        let id: String
        let request: ConsentRequest
        let resume: @MainActor (ConsentDecision) -> Void
    }

    var pending: Pending?
    private var queue: [Pending] = []

    func ask(_ request: ConsentRequest) async -> ConsentDecision {
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                var resumed = false
                let entry = Pending(id: request.id, request: request) { decision in
                    guard !resumed else { return }
                    resumed = true
                    continuation.resume(returning: decision)
                }
                if Task.isCancelled {
                    entry.resume(.declined)
                    return
                }
                if pending == nil {
                    pending = entry
                } else {
                    queue.append(entry)
                }
            }
        } onCancel: {
            Task { @MainActor in self.cancel(request.id) }
        }
    }

    func decide(_ pending: Pending, _ decision: ConsentDecision) {
        advance(pending.id)
        pending.resume(decision)
    }

    private func cancel(_ id: String) {
        let entry = pending?.id == id ? pending : queue.first { $0.id == id }
        guard let entry else { return }
        advance(id)
        entry.resume(.declined)
    }

    private func advance(_ id: String) {
        if pending?.id == id {
            pending = queue.isEmpty ? nil : queue.removeFirst()
        } else {
            queue.removeAll { $0.id == id }
        }
    }
}

struct ConsentSheet: View {
    let pending: ConsentCoordinator.Pending
    let onDecide: (ConsentDecision) -> Void
    @State private var dontAskAgain = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                detail
                    .padding(.horizontal, Design.Space.gutter)
                    .padding(.vertical, Design.Space.xl)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            footer
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.l)
        }
        .frame(width: 640, height: 560)
        .interactiveDismissDisabled()
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: glyph)
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(title)
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text(subtitle)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
        }
        .padding(.horizontal, Design.Space.gutter)
        .padding(.vertical, Design.Space.l)
    }

    @ViewBuilder
    private var detail: some View {
        switch pending.request.kind {
        case .write(let path, let diff, let foreign):
            VStack(alignment: .leading, spacing: Design.Space.m) {
                MicroHeader(title: "Write")
                Text(path)
                    .font(Design.data(12))
                    .foregroundStyle(Design.ink)
                    .textSelection(.enabled)
                if let foreign {
                    Text(foreign)
                        .font(Design.caption)
                        .foregroundStyle(Design.heat)
                }
                codeBlock(diff)
            }
        case .command(let argv, let timeoutSeconds):
            VStack(alignment: .leading, spacing: Design.Space.m) {
                MicroHeader(title: "Run command")
                Text("Times out after \(timeoutSeconds)s. No network. Confined to this folder.")
                    .font(Design.caption)
                    .foregroundStyle(Design.inkFaint)
                codeBlock(argv.joined(separator: "\n"))
            }
        }
    }

    private func codeBlock(_ text: String) -> some View {
        Text(text)
            .font(Design.data(12))
            .foregroundStyle(Design.ink)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(Design.Space.l)
            .background(Design.inkWash, in: RoundedRectangle(cornerRadius: Design.Radius.surface))
    }

    private var footer: some View {
        HStack(spacing: Design.Space.l) {
            Toggle(isOn: $dontAskAgain) {
                Text("Don't ask again for \(pending.request.toolName) this conversation")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            .toggleStyle(.checkbox)
            Spacer()
            Button("Decline") { onDecide(.declined) }
                .buttonStyle(QuietButtonStyle())
            Button("Approve") { onDecide(.approved(dontAskAgain: dontAskAgain)) }
                .buttonStyle(InkButtonStyle())
        }
    }

    private var glyph: String {
        switch pending.request.kind {
        case .write: "square.and.pencil"
        case .command: "terminal"
        }
    }

    private var title: String {
        switch pending.request.kind {
        case .write: "Write to a file"
        case .command: "Run a command"
        }
    }

    private var subtitle: String {
        "The model is asking to change your machine — approve the exact action below"
    }
}
