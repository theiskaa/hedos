import HedosKernel
import SwiftUI

struct HomePane: View {
    @Bindable var shell: ShellModel
    @State private var artifacts: [Artifact] = []

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Design.Space.pane) {
                hero
                if let failure = shell.library.errorMessage {
                    scanFailure(failure)
                } else if let summary, summary.totalCount == 0 {
                    FirstRunDiscovery(shell: shell)
                } else if summary == nil {
                    lookingLine
                } else {
                    board
                    if !readyModels.isEmpty {
                        readySection
                    }
                    if !riverItems.isEmpty {
                        continueRiver
                    }
                }
                hints
            }
            .padding(.horizontal, Design.Space.gutter + Design.Space.m)
            .padding(.top, Design.Space.pane + Design.Space.l)
            .padding(.bottom, Design.Space.pane)
            .frame(maxWidth: Design.Column.hero, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(alignment: .topTrailing) {
            HedosLogo(size: 360, color: Design.ink)
                .opacity(0.05)
                .padding(.trailing, -60)
                .padding(.top, Design.Space.gutter)
                .allowsHitTesting(false)
        }
        .background(PixelGrid())
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

    private var hero: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                HedosWordmark(unit: 9, color: Design.ink)
                Text("A home for every local model on your machine.")
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkSoft)
            }
            Spacer(minLength: 0)
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
    }

    private var hints: some View {
        Text(hintLine.uppercased())
            .font(Design.micro)
            .tracking(Design.microTracking)
            .foregroundStyle(Design.inkFaint)
    }

    private var hintLine: String {
        (ShellModel.surfaces + [.library])
            .map { "\(Design.modeTitle($0)) ⌘\($0.ordinal)" }
            .joined(separator: " · ")
    }

    private var lookingLine: some View {
        Text("Looking around this Mac…")
            .font(Design.title)
            .foregroundStyle(Design.inkSoft)
    }

    private var board: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            statCard
            VStack(alignment: .leading, spacing: Design.Space.l) {
                temperatureCard
                warmNowCard
            }
        }
    }

    @ViewBuilder
    private var statCard: some View {
        if let summary {
            VStack(alignment: .leading, spacing: Design.Space.xl) {
                HStack {
                    MicroHeader(title: "On this machine")
                    Spacer(minLength: 0)
                    if shell.library.isScanning {
                        ShimmerText(text: "Scanning…".uppercased())
                    }
                }
                HStack(alignment: .bottom, spacing: Design.Space.l) {
                    PixelNumber(text: "\(summary.totalCount)", unit: 6, color: Design.ink)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("models found".uppercased())
                            .font(Design.micro)
                            .tracking(Design.microTracking)
                            .foregroundStyle(Design.inkFaint)
                        Text(DiscoverySummary.formatBytes(summary.totalBytes))
                            .font(Design.data(12))
                            .foregroundStyle(Design.inkSoft)
                    }
                    Spacer(minLength: 0)
                }
                if shell.residencyBudgetMB > 0 {
                    VStack(alignment: .leading, spacing: Design.Space.s) {
                        HStack {
                            Text("memory budget".uppercased())
                                .font(Design.label)
                                .tracking(Design.microTracking)
                                .foregroundStyle(Design.inkFaint)
                            Spacer(minLength: 0)
                            Text(
                                "\(DiscoverySummary.formatBytes(Int64(shell.residentUsedMB) << 20)) / \(DiscoverySummary.formatBytes(Int64(shell.residencyBudgetMB) << 20))"
                            )
                            .font(Design.data(10))
                            .monospacedDigit()
                            .foregroundStyle(Design.inkFaint)
                        }
                        SegmentedBar(used: residentFraction, warm: residentFraction, segments: 24)
                            .animation(Design.spring, value: shell.residentUsedMB)
                    }
                }
                systemStats
                if let pick = Fit.recommendation(in: shell.library.records),
                    pick.fit?.verdict != .tooLarge
                {
                    Button("Start chat") {
                        shell.startChat(bound: pick)
                    }
                    .buttonStyle(InkButtonStyle())
                    .accessibilityIdentifier("home-recommendation")
                }
            }
            .padding(Design.Space.xl)
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.card)
        }
    }

    private var warmNowCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Warm now · \(shell.resident.count)")
            if shell.resident.isEmpty {
                Text("Nothing warm. Models sleep until you ask.")
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkFaint)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                ForEach(shell.resident, id: \.self) { entry in
                    HStack(spacing: Design.Space.chipX) {
                        AccentDot()
                        Text(residentName(entry))
                            .font(Design.body.weight(.medium))
                            .foregroundStyle(Design.ink)
                            .lineLimit(1)
                        Spacer(minLength: Design.Space.m)
                        Text(DiscoverySummary.formatBytes(Int64(entry.footprintMB) << 20))
                            .font(Design.data(11))
                            .monospacedDigit()
                            .foregroundStyle(Design.inkFaint)
                    }
                }
            }
        }
        .padding(Design.Space.xl)
        .frame(width: 288, alignment: .leading)
        .surfaceCard(radius: Design.Radius.card)
    }

    private var temperatureCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                MicroHeader(title: "Temperature")
                Spacer(minLength: 0)
                Text(shell.system.thermalLabel.uppercased())
                    .font(Design.label)
                    .tracking(Design.microTracking)
                    .foregroundStyle(temperatureColor)
            }
            HStack(alignment: .center, spacing: Design.Space.m) {
                Text(temperatureText)
                    .font(Design.paneTitle)
                    .monospacedDigit()
                    .foregroundStyle(temperatureColor)
                Spacer(minLength: 0)
                HStack(spacing: 3) {
                    ForEach(0..<3, id: \.self) { index in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(temperatureSegment(index))
                            .frame(width: 10, height: 10)
                    }
                }
            }
        }
        .padding(Design.Space.xl)
        .frame(width: 288, alignment: .leading)
        .surfaceCard(radius: Design.Radius.card)
    }

    private var temperatureLevel: Int {
        if let celsius = shell.system.temperatureC {
            if celsius >= 80 { return 2 }
            if celsius >= 65 { return 1 }
            return 0
        }
        switch shell.system.thermal {
        case .serious, .critical: return 2
        case .fair: return 1
        default: return 0
        }
    }

    private var temperatureColor: Color {
        switch temperatureLevel {
        case 2: Design.danger
        case 1: Design.heat
        default: Design.accentText
        }
    }

    private func temperatureSegment(_ index: Int) -> Color {
        guard index <= temperatureLevel else { return Design.line }
        switch index {
        case 0: return Design.accentText
        case 1: return Design.heat
        default: return Design.danger
        }
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
            HStack(alignment: .bottom, spacing: Design.Space.l) {
                PixelNumber(text: "\(summary.totalCount)", unit: 6, color: Design.ink)
                VStack(alignment: .leading, spacing: 3) {
                    Text("models on this Mac".uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    HStack(spacing: Design.Space.s) {
                        Text(DiscoverySummary.formatBytes(summary.totalBytes))
                            .font(Design.data(12))
                            .foregroundStyle(Design.inkSoft)
                        if !shell.resident.isEmpty {
                            Text("· \(shell.resident.count) warm")
                                .font(Design.data(12))
                                .foregroundStyle(Design.heatText)
                            AccentDot(size: 8)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .animation(Design.spring, value: summary.totalCount)
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
            .foregroundStyle(Design.heatText)
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

    private var residentFraction: Double {
        min(Double(shell.residentUsedMB) / Double(max(1, shell.residencyBudgetMB)), 1)
    }

    private var heatBar: some View {
        HStack(spacing: Design.Space.chipX) {
            SegmentedBar(used: residentFraction, warm: residentFraction, segments: 20)
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

    @ViewBuilder
    private var systemStats: some View {
        if let mem = shell.system.memory {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                HStack(spacing: Design.Space.m) {
                    Text("system memory".uppercased())
                        .font(Design.label)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    Spacer(minLength: 0)
                    Text(
                        "\(DiscoverySummary.formatBytes(Int64(mem.usedBytes))) / \(DiscoverySummary.formatBytes(Int64(mem.totalBytes)))"
                    )
                    .font(Design.data(10))
                    .monospacedDigit()
                    .foregroundStyle(Design.inkFaint)
                }
                SegmentedBar(used: mem.usedFraction, warm: 0, segments: 24)
                    .animation(Design.spring, value: mem.usedBytes)
            }
        }
    }

    private var temperatureTag: some View {
        HStack(spacing: Design.Space.xs) {
            RoundedRectangle(cornerRadius: 1)
                .fill(shell.system.runningHot ? Design.heat : Design.inkFaint)
                .frame(width: 7, height: 7)
            Text(temperatureText)
                .font(Design.data(10))
                .monospacedDigit()
                .foregroundStyle(shell.system.runningHot ? Design.heatText : Design.inkSoft)
        }
    }

    private var temperatureText: String {
        if let celsius = shell.system.temperatureC {
            return String(format: "%.0f°C", celsius)
        }
        return "—"
    }

    private static let readyGridLimit = 6

    private var readyModels: [ModelRecord] {
        shell.library.records
            .filter {
                $0.state == .ready && $0.runtime.tier != .recipeNeeded
                    && Launcher.destination(for: $0) != .library
            }
            .sorted { first, second in
                if Fit.rank(first) != Fit.rank(second) {
                    return Fit.rank(first) < Fit.rank(second)
                }
                return (first.footprintMB ?? 0) > (second.footprintMB ?? 0)
            }
    }

    private var readySection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                MicroHeader(title: "Ready to run · \(readyModels.count)")
                Spacer(minLength: 0)
                if readyModels.count > Self.readyGridLimit {
                    Button("See all") {
                        shell.modelsFilter = ModelFilter(statuses: [.ready])
                        shell.setMode(.library)
                    }
                    .buttonStyle(QuietButtonStyle())
                    .accessibilityIdentifier("home-see-all-models")
                }
            }
            LazyVGrid(
                columns: [
                    GridItem(.adaptive(minimum: 200), spacing: Design.Space.l, alignment: .top)
                ],
                spacing: Design.Space.l
            ) {
                ForEach(readyModels.prefix(Self.readyGridLimit)) { record in
                    ReadyModelCard(record: record, warm: isWarm(record)) {
                        shell.launch(record)
                    }
                }
            }
        }
    }

    private func isWarm(_ record: ModelRecord) -> Bool {
        shell.resident.contains { $0.modelID == record.id || $0.name == record.name }
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
                let text = SpeechArtifact.text(of: artifact)
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
                shell.setMode(.chat)
            } else {
                shell.showArtifact(artifact.id)
            }
        }
    }
}

struct ReadyModelCard: View {
    let record: ModelRecord
    var warm = false
    let onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: record.source.kind, size: 16)
                        .foregroundStyle(Design.inkSoft)
                        .frame(width: 18, height: 18)
                        .overlay(alignment: .topTrailing) {
                            if warm {
                                AccentDot(size: 6)
                                    .offset(x: 3, y: -3)
                            }
                        }
                    Text(record.displayName)
                        .font(Design.body.weight(.medium))
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                        .truncationMode(.tail)
                    Spacer(minLength: 0)
                }
                HStack(spacing: Design.Space.s) {
                    TintChip(
                        text: Design.modeTitle(destination),
                        glyph: Design.modeGlyph(destination))
                    FitChip(record: record)
                }
                HStack(alignment: .firstTextBaseline) {
                    Text(
                        record.footprintMB.map {
                            $0 > 0 ? DiscoverySummary.formatBytes(Int64($0) << 20) : "—"
                        } ?? "size unknown"
                    )
                    .font(Design.data(11))
                    .monospacedDigit()
                    .foregroundStyle(Design.inkSoft)
                    Spacer(minLength: 0)
                }
            }
            .padding(Design.Space.l)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(Design.surface, in: RoundedRectangle(cornerRadius: Design.Radius.tile))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.tile)
                    .strokeBorder(
                        hovering
                            ? AnyShapeStyle(Design.accentEdge)
                            : warm ? AnyShapeStyle(Design.lineBright) : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.tile))
            .lifts(hovering: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help("Open \(record.displayName) in \(Design.modeTitle(destination))")
        .accessibilityLabel(record.displayName)
        .accessibilityIdentifier("ready-model-\(record.id)")
    }

    private var destination: AppMode {
        Launcher.destination(for: record)
    }
}
