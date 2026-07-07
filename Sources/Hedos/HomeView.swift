import HedosKernel
import SwiftUI

struct HomePane: View {
    @Bindable var shell: ShellModel
    @State private var artifacts: [Artifact] = []

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Home") {
                if shell.library.isScanning {
                    ProgressView()
                        .controlSize(.small)
                }
                QuietIconButton(glyph: "arrow.clockwise") {
                    Task { await shell.library.rescan() }
                }
                .disabled(shell.library.isScanning)
                .help("Scan the machine again")
                .accessibilityLabel("Rescan")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.pane) {
                    statusBlock
                    if let failure = shell.library.errorMessage {
                        scanFailure(failure)
                    } else if let summary, summary.totalCount == 0 {
                        coldStartCard
                    } else {
                        startCard
                        if !shell.resident.isEmpty {
                            warmNow
                        }
                        if !riverItems.isEmpty {
                            continueRiver
                        }
                    }
                    Text("Chat ⌘1 · Images ⌘2 · Voice ⌘3 · Models ⌘4".uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
                .padding(.horizontal, Design.Space.gutter + Design.Space.m)
                .padding(.top, Design.Space.pane)
                .padding(.bottom, Design.Space.pane)
                .frame(maxWidth: Design.Column.hero, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task {
            if shell.library.summary == nil {
                await shell.library.rescan()
            }
            artifacts = (try? await shell.kernel.artifacts()) ?? []
        }
        .task(id: shell.sessions.count) {
            artifacts = (try? await shell.kernel.artifacts()) ?? []
        }
    }

    private var summary: DiscoverySummary? {
        shell.library.summary
    }

    private var statusBlock: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            HStack(spacing: Design.Space.m) {
                Text("Machine".uppercased())
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(Design.inkFaint)
                if shell.library.isScanning {
                    ShimmerText(text: "Scanning…".uppercased())
                }
            }
            statusLine
        }
    }

    @ViewBuilder
    private var statusLine: some View {
        if let summary, summary.totalCount > 0 {
            HStack(alignment: .center, spacing: Design.Space.l) {
                Text("\(counts(summary))\(warmSegment)")
                    .font(Design.display)
                    .tracking(Design.tightTracking)
                    .monospacedDigit()
                    .foregroundStyle(Design.ink)
                    .contentTransition(.numericText())
                    .animation(Design.spring, value: summary.totalCount)
                    .animation(Design.spring, value: shell.resident.count)
                if !shell.resident.isEmpty {
                    AccentDot(size: 9)
                }
            }
        } else if let summary, summary.totalCount == 0 {
            Text("Nothing on this Mac speaks yet.")
                .font(Design.display)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
        } else {
            Text("Looking around this Mac…")
                .font(Design.display)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.inkSoft)
        }
    }

    private func counts(_ summary: DiscoverySummary) -> Text {
        Text(
            "\(summary.totalCount) \(summary.totalCount == 1 ? "model" : "models") · \(DiscoverySummary.formatBytes(summary.totalBytes))"
        )
    }

    private var warmSegment: Text {
        guard !shell.resident.isEmpty else { return Text(verbatim: "") }
        return Text(" · \(shell.resident.count) warm")
            .foregroundStyle(Design.accentText)
    }

    private func scanFailure(_ failure: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            Text("The scan hit a problem.")
                .font(Design.title)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
            Text(failure)
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(Design.bodyLineSpacing)
                .frame(maxWidth: Design.Column.prose, alignment: .leading)
            Button("Scan again") {
                Task { await shell.library.rescan() }
            }
            .buttonStyle(InkButtonStyle())
        }
        .padding(Design.Space.xxl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private var coldStartCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            Text("Models put here by Ollama, LM Studio, or the Hugging Face cache appear on their own. For weights that live anywhere else, point Hedos at the folder.")
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(Design.bodyLineSpacing)
                .frame(maxWidth: Design.Column.prose, alignment: .leading)
            Button("Watch a folder…") {
                shell.settingsTarget = SettingsDestination(
                    section: .models, anchor: "models.folders")
                SettingsWindowController.shared.show(shell: shell)
            }
            .buttonStyle(InkButtonStyle())
        }
        .padding(Design.Space.xxl)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.tile)
    }

    @ViewBuilder
    private var startCard: some View {
        if let pick = Fit.recommendation(in: shell.library.records),
            pick.fit?.verdict != .tooLarge
        {
            HStack(alignment: .center, spacing: Design.Space.xl) {
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    Text("\(pick.displayName) fits this Mac best.")
                        .font(Design.title)
                        .tracking(Design.tightTracking)
                        .foregroundStyle(Design.ink)
                    Text(startCardSubtitle(pick))
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkFaint)
                }
                Spacer(minLength: Design.Space.xl)
                Button("Start chat") {
                    shell.startChat(bound: pick)
                }
                .buttonStyle(InkButtonStyle())
                .accessibilityIdentifier("home-recommendation")
            }
            .padding(Design.Space.xxl)
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.tile)
        } else if Launcher.defaultChatModel(in: shell.library.records) != nil {
            HStack(alignment: .center, spacing: Design.Space.xl) {
                Text("A chat model is ready.")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                Spacer(minLength: Design.Space.xl)
                Button("Start chat") {
                    shell.newChat()
                }
                .buttonStyle(InkButtonStyle())
            }
            .padding(Design.Space.xxl)
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private func startCardSubtitle(_ pick: ModelRecord) -> String {
        var parts: [String] = []
        if let mb = pick.footprintMB, mb > 0 {
            parts.append(DiscoverySummary.formatBytes(Int64(mb) << 20))
        }
        if let runtime = pick.runtime.id {
            parts.append(runtime)
        }
        parts.append("ready")
        return parts.joined(separator: " · ")
    }

    private var warmNow: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Warm now")
            VStack(alignment: .leading, spacing: Design.Space.s) {
                ForEach(shell.resident, id: \.self) { entry in
                    HStack(spacing: Design.Space.chipX) {
                        AccentDot()
                        Text(residentName(entry))
                            .font(Design.body.weight(.medium))
                            .foregroundStyle(Design.ink)
                            .lineLimit(1)
                        Text(DiscoverySummary.formatBytes(Int64(entry.footprintMB) << 20))
                            .font(Design.data(11))
                            .foregroundStyle(Design.inkFaint)
                        Spacer(minLength: 0)
                    }
                }
                if shell.residencyBudgetMB > 0 {
                    heatBar
                        .padding(.top, Design.Space.xs)
                }
            }
            .padding(Design.Space.xl)
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private var heatBar: some View {
        HStack(spacing: Design.Space.chipX) {
            GeometryReader { geometry in
                let width = geometry.size.width
                let fraction = min(
                    Double(shell.residentUsedMB) / Double(max(1, shell.residencyBudgetMB)), 1)
                ZStack(alignment: .leading) {
                    Capsule()
                        .fill(Design.line)
                    Capsule()
                        .fill(Design.accent)
                        .frame(width: max(5, width * fraction))
                }
            }
            .frame(height: 5)
            .animation(Design.spring, value: shell.residentUsedMB)
            Text(
                "\(DiscoverySummary.formatBytes(Int64(shell.residentUsedMB) << 20)) / \(DiscoverySummary.formatBytes(Int64(shell.residencyBudgetMB) << 20))"
            )
            .font(Design.data(10))
            .monospacedDigit()
            .foregroundStyle(Design.inkFaint)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(
            "Memory: \(shell.residentUsedMB >> 10) of \(shell.residencyBudgetMB >> 10) gigabytes warm"
        )
    }

    private func residentName(_ entry: Kernel.ResidentEntry) -> String {
        if let id = entry.modelID, let record = shell.library.record(id: id) {
            return record.displayName
        }
        return entry.name
    }

    private enum RiverItem: Identifiable {
        case chat(ChatSession)
        case artifact(Artifact)

        var id: String {
            switch self {
            case .chat(let session): "chat-\(session.id)"
            case .artifact(let artifact): "artifact-\(artifact.id)"
            }
        }

        var date: Date {
            switch self {
            case .chat(let session): session.updatedAt
            case .artifact(let artifact): artifact.createdAt
            }
        }
    }

    private var riverItems: [RiverItem] {
        let chats = shell.filteredSessions.map(RiverItem.chat)
        let made = artifacts
            .filter { $0.capability == .image || $0.capability == .speak }
            .map(RiverItem.artifact)
        return (chats + made).sorted { $0.date > $1.date }.prefix(8).map { $0 }
    }

    private var continueRiver: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Continue")
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                ForEach(riverItems) { item in
                    riverRow(item)
                }
            }
        }
    }

    private func riverRow(_ item: RiverItem) -> some View {
        Button {
            open(item)
        } label: {
            HStack(spacing: Design.Space.chipX) {
                Image(systemName: riverGlyph(item))
                    .font(Design.glyphInline)
                    .foregroundStyle(Design.inkSoft)
                    .frame(width: 18, alignment: .leading)
                Text(riverTitle(item))
                    .font(Design.body)
                    .foregroundStyle(Design.ink)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: Design.Space.l)
                Text(item.date.formatted(.relative(presentation: .named)))
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s + 1)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .fill(hoveredRiverItem == item.id ? Design.inkWash : .clear))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .onHover { inside in
            if inside {
                hoveredRiverItem = item.id
            } else if hoveredRiverItem == item.id {
                hoveredRiverItem = nil
            }
        }
        .animation(Design.wash, value: hoveredRiverItem)
        .accessibilityLabel(riverTitle(item))
    }

    @State private var hoveredRiverItem: String?

    private func riverGlyph(_ item: RiverItem) -> String {
        switch item {
        case .chat: "message"
        case .artifact(let artifact):
            artifact.capability == .speak ? "waveform" : "photo"
        }
    }

    private func riverTitle(_ item: RiverItem) -> String {
        switch item {
        case .chat(let session):
            return session.title.isEmpty ? "Untitled chat" : session.title
        case .artifact(let artifact):
            if artifact.capability == .speak {
                let text = VoiceSurfaceModel.text(of: artifact)
                return text.isEmpty ? "Narration" : text
            }
            return Provenance.prompt(of: artifact.params) ?? "Untitled image"
        }
    }

    private func open(_ item: RiverItem) {
        switch item {
        case .chat(let session):
            shell.selectChat(session.id)
            shell.setMode(.chat)
        case .artifact(let artifact):
            if artifact.capability == .speak {
                shell.setMode(.voice)
            } else {
                shell.showArtifact(artifact.id)
            }
        }
    }
}
