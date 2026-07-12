import HedosKernel
import SwiftUI

struct HomePane: View {
    @Bindable var shell: ShellModel
    @State private var chatDraft = ""
    @State private var denyCount = 0
    @FocusState private var chatFieldFocused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var subtitle = HomePane.subtitles.randomElement() ?? HomePane.subtitles[0]

    private static let subtitles = [
        "Control isn't an illusion here — every model runs on your machine, and nothing leaves it.",
        "No cloud, no watchers — just you and the machines you own.",
        "Every model runs local. Nothing you say leaves this Mac.",
        "Private by design. Everything happens right here, on your metal.",
        "Your machine, your models, your rules.",
        "The only server here is the one on your desk.",
    ]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Design.Space.pane) {
                topStrip
                if let summary, summary.totalCount == 0 {
                    FirstRunDiscovery(shell: shell)
                        .transition(.opacity)
                } else if summary == nil, let failure = shell.library.errorMessage {
                    scanFailure(failure)
                        .transition(.opacity)
                } else {
                    Group {
                        centeredHero
                        board
                        if !readyModels.isEmpty {
                            readySection
                        }
                    }
                    .transition(.opacity)
                }
                hints
            }
            .padding(.horizontal, Design.Space.gutter + Design.Space.m)
            .padding(.top, Design.Space.pane + Design.Space.l)
            .padding(.bottom, Design.Space.pane)
            .frame(maxWidth: Design.Column.hero, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
            .animation(Design.motion(reduceMotion: reduceMotion), value: summary?.totalCount)
            .animation(Design.motion(reduceMotion: reduceMotion), value: summary == nil)
        }
        .background(alignment: .topTrailing) {
            HedosLogo(size: 360, color: Design.ink)
                .opacity(0.05)
                .padding(.trailing, -60)
                .padding(.top, Design.Space.gutter)
                .allowsHitTesting(false)
        }
        .background(PixelGrid())
        .onAppear { shell.system.start() }
        .onDisappear { shell.system.stop() }
        .task {
            await shell.refreshSessions()
            if shell.library.summary == nil {
                await shell.library.rescan()
            }
        }
    }

    private var summary: DiscoverySummary? {
        shell.library.summary
    }

    private var topStrip: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            Spacer(minLength: 0)
            if summary != nil {
                temperatureBadge
            }
            if shell.library.isScanning {
                ProgressView()
                    .controlSize(.small)
                    .transition(.opacity)
            }
            QuietIconButton(glyph: "arrow.clockwise") {
                Task { await shell.library.rescan() }
            }
            .disabled(shell.library.isScanning)
            .help("Scan the machine again")
            .accessibilityLabel("Rescan")
        }
        .animation(Design.motion(reduceMotion: reduceMotion), value: shell.library.isScanning)
    }

    private var temperatureBadge: some View {
        HStack(spacing: Design.Space.s) {
            Image(systemName: "thermometer.medium")
                .font(Design.glyphInline)
            Text(temperatureText)
                .font(Design.caption.weight(.medium))
                .monospacedDigit()
        }
        .foregroundStyle(temperatureColor)
        .padding(.horizontal, Design.Space.chipX)
        .padding(.vertical, Design.Space.s)
        .background(Design.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .help("System temperature · \(shell.system.thermalLabel)")
    }

    private var centeredHero: some View {
        VStack(spacing: Design.Space.l) {
            VStack(spacing: Design.Space.s) {
                Text(greeting)
                    .font(Design.hero)
                    .foregroundStyle(Design.ink)
                    .multilineTextAlignment(.center)
                Text(subtitle)
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkSoft)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 440)
            }
            startChatField
            Text(
                defaultChatName.map { "New chats use \($0) · runs local, stays private" }
                    ?? "Runs local · stays private"
            )
            .font(Design.caption)
            .foregroundStyle(Design.inkFaint)
        }
        .frame(maxWidth: 560)
        .frame(maxWidth: .infinity)
        .padding(.top, Design.Space.l)
        .padding(.bottom, Design.Space.m)
    }

    private var startChatField: some View {
        HStack(alignment: .center, spacing: Design.Space.m) {
            TextField("Start a chat…", text: $chatDraft)
                .textFieldStyle(.plain)
                .font(Design.body)
                .focused($chatFieldFocused)
                .onSubmit(launchChat)
            CircleControl(
                glyph: "arrow.up",
                prominent: !chatDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && defaultChatName != nil,
                label: "Start chat",
                action: launchChat
            )
            .disabled(defaultChatName == nil)
        }
        .padding(.leading, Design.Space.xl)
        .padding(.trailing, Design.Space.s)
        .padding(.vertical, Design.Space.s)
        .surfaceCard(radius: Design.Radius.bubble)
        .denyShake(on: denyCount, in: RoundedRectangle.soft(Design.Radius.bubble))
        .onTapGesture { chatFieldFocused = true }
    }

    private var greeting: String {
        let hour = Calendar.current.component(.hour, from: Date())
        switch hour {
        case 5..<12: return "Good morning."
        case 12..<17: return "Good afternoon."
        case 17..<22: return "Good evening."
        default: return "Hello, friend."
        }
    }

    private var defaultChatName: String? {
        Launcher.defaultChatModel(
            in: shell.library.records, preferring: shell.preferredChatModelID
        )?.displayName
    }

    private func launchChat() {
        let text = chatDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            denyCount += 1
            chatFieldFocused = true
            return
        }
        guard let record = Launcher.defaultChatModel(
            in: shell.library.records, preferring: shell.preferredChatModelID)
        else { return }
        shell.startChat(bound: record, seed: text)
        chatDraft = ""
    }

    private var hints: some View {
        Text(hintLine)
            .font(Design.micro)
            .foregroundStyle(Design.inkFaint)
    }

    private var hintLine: String {
        let paged = AppMode.allCases
            .filter { $0 != .settings && ShellModel.surfaced($0) == $0 }
            .enumerated()
            .map { "\(Design.modeTitle($1)) ⌘\($0 + 1)" }
        return (paged + ["Settings ⌘,"]).joined(separator: " · ")
    }

    private var board: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            statCard
                .staggeredArrival(0)
            warmNowCard
                .staggeredArrival(1)
        }
    }

    @ViewBuilder
    private var statCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.xl) {
            HStack {
                MicroHeader(title: "On this machine")
                Spacer(minLength: 0)
                if shell.library.isScanning {
                    ShimmerText(text: "Scanning…", tracked: false)
                }
            }
            HStack(alignment: .bottom, spacing: Design.Space.l) {
                Group {
                    if let summary {
                        PixelNumber(text: "\(summary.totalCount)", unit: 6, color: Design.ink)
                            .transition(.opacity)
                    } else {
                        SkeletonPulse(radius: Design.Radius.control)
                            .frame(width: 48, height: 6 * 7)
                            .transition(.opacity)
                    }
                }
                .frame(width: 84, height: 6 * 7, alignment: .bottomLeading)
                .animation(Design.motion(reduceMotion: reduceMotion), value: summary == nil)
                VStack(alignment: .leading, spacing: 3) {
                    Text("models found")
                        .font(Design.micro)
                        .foregroundStyle(Design.inkFaint)
                    if let summary {
                        Text(DiscoverySummary.formatBytes(summary.totalBytes))
                            .font(Design.data(12))
                            .foregroundStyle(Design.inkSoft)
                    } else {
                        Text(verbatim: "000 MB")
                            .font(Design.data(12))
                            .foregroundStyle(.clear)
                            .overlay(SkeletonPulse(radius: Design.Radius.control))
                    }
                }
                Spacer(minLength: 0)
            }
            if shell.residencyBudgetMB > 0 {
                VStack(alignment: .leading, spacing: Design.Space.s) {
                    HStack {
                        Text("memory budget")
                            .font(Design.label)
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
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard(radius: Design.Radius.card)
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
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
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
            parts.append(runtime.rawValue)
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
        shell.isWarm(record)
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
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
            .overlay(
                RoundedRectangle.soft(Design.Radius.tile)
                    .strokeBorder(
                        hovering
                            ? AnyShapeStyle(Design.accentEdge)
                            : warm ? AnyShapeStyle(Design.lineBright) : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(Design.Radius.tile))
            .lifts(hovering: hovering)
        }
        .buttonStyle(PressDipStyle())
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
