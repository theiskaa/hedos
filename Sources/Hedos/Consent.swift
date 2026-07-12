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

struct ConsentCard: View {
    let pending: ConsentCoordinator.Pending
    let onDecide: (ConsentDecision) -> Void
    @State private var dontAskAgain = false
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            header
            if expanded {
                ScrollView {
                    detail
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.trailing, Design.Space.xs)
                }
                .frame(maxHeight: 220)
            }
            footer
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var header: some View {
        HStack(spacing: Design.Space.m) {
            Image(systemName: glyph)
                .font(Design.glyphNav)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 22)
            Text(intent)
                .font(Design.body.weight(.medium))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: Design.Space.m)
            Button {
                withAnimation(Design.spring) { expanded.toggle() }
            } label: {
                HStack(spacing: Design.Space.xs) {
                    Text(expanded ? "Hide details" : "Show details")
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(Design.glyphSmall)
                }
                .font(Design.label.weight(.medium))
                .foregroundStyle(Design.inkFaint)
            }
            .buttonStyle(.plain)
        }
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
                diffBlock(diff)
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
            .background(Design.inkWash, in: RoundedRectangle.soft(Design.Radius.surface))
    }

    private func diffBlock(_ diff: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(diff.split(separator: "\n", omittingEmptySubsequences: false).enumerated()), id: \.offset) { _, raw in
                let line = String(raw)
                Text(line.isEmpty ? " " : line)
                    .font(Design.data(12))
                    .foregroundStyle(diffColor(line))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .textSelection(.enabled)
        .padding(Design.Space.l)
        .background(Design.inkWash, in: RoundedRectangle.soft(Design.Radius.surface))
    }

    private func diffColor(_ line: String) -> Color {
        if line.hasPrefix("+++") || line.hasPrefix("---") || line.hasPrefix("@@") {
            return Design.inkFaint
        }
        if line.hasPrefix("+") { return Design.added }
        if line.hasPrefix("-") { return Design.danger }
        return Design.inkSoft
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
            Button("Deny") { onDecide(.declined) }
                .buttonStyle(QuietButtonStyle())
                .keyboardShortcut(.cancelAction)
            Button("Approve") { onDecide(.approved(dontAskAgain: dontAskAgain)) }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
        }
    }

    private var glyph: String {
        switch pending.request.kind {
        case .write: "square.and.pencil"
        case .command: "terminal"
        }
    }

    private var intent: String {
        switch pending.request.kind {
        case .write(let path, _, _): "Write to \(path)"
        case .command(let argv, _): "Run \(argv.joined(separator: " "))"
        }
    }
}
