import AppKit
import HedosKernel
import SwiftUI

enum ModelStatus: String, Hashable, CaseIterable {
    case ready
    case fits
    case warm
    case needsRecipe
    case missing

    var label: String {
        switch self {
        case .ready: "Ready"
        case .fits: "Fits"
        case .warm: "Warm"
        case .needsRecipe: "Needs recipe"
        case .missing: "Missing"
        }
    }

    func matches(_ record: ModelRecord, warm: Bool) -> Bool {
        switch self {
        case .ready:
            record.state == .ready && record.runtime.tier != .recipeNeeded
        case .fits:
            record.fit?.verdict == .runsWell || record.fit?.verdict == .tightFit
        case .warm:
            warm
        case .needsRecipe:
            record.runtime.tier == .recipeNeeded
        case .missing:
            record.state == .missing
        }
    }
}

struct ModelFilter: Equatable {
    var capabilities: Set<AppMode> = []
    var sources: Set<String> = []
    var statuses: Set<ModelStatus> = []

    var isEmpty: Bool {
        capabilities.isEmpty && sources.isEmpty && statuses.isEmpty
    }

    static func hasCapability(_ record: ModelRecord, _ mode: AppMode) -> Bool {
        !Launcher.models(in: [record], for: mode).isEmpty
    }

    func matches(_ record: ModelRecord, warm: Bool) -> Bool {
        if !capabilities.isEmpty,
            !capabilities.contains(where: { Self.hasCapability(record, $0) })
        {
            return false
        }
        if !sources.isEmpty, !sources.contains(ModelsPane.storeTitle(record.source.kind)) {
            return false
        }
        if !statuses.isEmpty, !statuses.contains(where: { $0.matches(record, warm: warm) }) {
            return false
        }
        return true
    }
}

struct ModelChip<Value: Hashable>: Identifiable {
    let value: Value
    let label: String
    var mark: SourceKind?
    let count: Int

    var id: Value { value }
}

struct ModelsPane: View {
    @Bindable var shell: ShellModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var filter = ModelFilter()
    @State private var query = ""
    @State private var showFolders = false
    @State private var presented: String?

    private static let contentWidth: CGFloat = 1080

    var body: some View {
        Group {
            if shell.library.records.isEmpty {
                emptyPane
            } else {
                dashboard
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(PixelGrid())
        .onAppear { adoptRequestedFilter() }
        .onChange(of: shell.modelsFilter) { adoptRequestedFilter() }
        .modalScrim(
            isPresented: shell.library.record(id: presented) != nil,
            onDismiss: { presented = nil }
        ) {
            if let record = shell.library.record(id: presented) {
                ModelDetailSheet(record: record, shell: shell) {
                    presented = nil
                }
            }
        }
    }

    private var dashboard: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Design.Space.pane) {
                hero
                contextRow
                filterRow
                gridContent
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.pane)
            .frame(maxWidth: Self.contentWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
    }

    private var emptyPane: some View {
        VStack(spacing: 0) {
            HStack(spacing: Design.Space.s) {
                Spacer(minLength: 0)
                controls
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xxl)
            heroEmptyState
        }
    }

    @ViewBuilder
    private var controls: some View {
        Color.clear
            .frame(width: 16, height: 16)
            .overlay {
                if shell.library.isScanning {
                    ProgressView().controlSize(.small)
                }
            }
        QuietIconButton(glyph: "folder.badge.plus") {
            showFolders.toggle()
        }
        .help("Watched folders")
        .accessibilityLabel("Watched folders")
        .inkPopover(
            isPresented: $showFolders,
            width: Design.Popover.form.width,
            maxHeight: Design.Popover.menuMaxHeight
        ) {
            FoldersPopover(model: shell.library) {
                showFolders = false
                shell.openSettings(
                    at: SettingsDestination(section: .models, anchor: "models.folders"))
            }
        }
        QuietIconButton(glyph: "arrow.clockwise") {
            Task { await shell.library.rescan() }
        }
        .disabled(shell.library.isScanning)
        .help("Scan the machine again")
        .accessibilityLabel("Rescan")
    }

    private var hero: some View {
        VStack(alignment: .leading, spacing: Design.Space.gutter) {
            HStack(alignment: .top, spacing: Design.Space.l) {
                VStack(alignment: .leading, spacing: Design.Space.s) {
                    Text("Models")
                        .font(Design.hero)
                        .foregroundStyle(Design.ink)
                    Text("Everything installed on this Mac, ready when you are.")
                        .font(Design.readingBody)
                        .foregroundStyle(Design.inkSoft)
                }
                Spacer(minLength: 0)
                HStack(spacing: Design.Space.s) {
                    controls
                }
            }
            facts
        }
        .padding(.bottom, Design.Space.xs)
    }

    private var facts: some View {
        HStack(alignment: .bottom, spacing: Design.Space.l) {
            Group {
                if let summary = shell.library.summary {
                    PixelNumber(text: "\(summary.totalCount)", unit: 6, color: Design.ink)
                } else {
                    SkeletonPulse(radius: Design.Radius.control)
                        .frame(width: 48, height: 6 * 7)
                }
            }
            .frame(height: 6 * 7, alignment: .bottomLeading)
            VStack(alignment: .leading, spacing: 3) {
                Text("models found")
                    .font(Design.micro)
                    .foregroundStyle(Design.inkFaint)
                if let summary = shell.library.summary {
                    HStack(spacing: Design.Space.s) {
                        Text(DiscoverySummary.formatBytes(summary.totalBytes))
                            .font(Design.data(12))
                            .foregroundStyle(Design.inkSoft)
                        if !shell.resident.isEmpty {
                            Text("· \(shell.resident.count) warm")
                                .font(Design.data(12))
                                .foregroundStyle(Design.heatText)
                            AccentDot(size: 7)
                        }
                    }
                } else {
                    Text(verbatim: "000 MB")
                        .font(Design.data(12))
                        .foregroundStyle(.clear)
                        .overlay(SkeletonPulse(radius: Design.Radius.control))
                }
            }
            Spacer(minLength: 0)
            ScanningTag(active: shell.library.isScanning)
        }
        .animation(Design.spring, value: shell.library.summary?.totalCount)
    }

    @ViewBuilder
    private var contextRow: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            if let pick = Fit.recommendation(in: shell.library.records),
                pick.fit?.verdict != .tooLarge
            {
                BestFitCard(record: pick) {
                    presented = pick.id
                    shell.selectLibrary(pick.id)
                }
            }
            warmCard
        }
    }

    private var warmCard: some View {
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
                if shell.residencyBudgetMB > 0 {
                    SegmentedBar(used: residentFraction, warm: residentFraction, segments: 24)
                        .animation(Design.spring, value: shell.residentUsedMB)
                        .padding(.top, Design.Space.xs)
                }
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard(radius: Design.Radius.card)
    }

    private var residentFraction: Double {
        min(Double(shell.residentUsedMB) / Double(max(1, shell.residencyBudgetMB)), 1)
    }

    private func residentName(_ entry: Kernel.ResidentEntry) -> String {
        if let id = entry.modelID, let record = shell.library.record(id: id) {
            return record.displayName
        }
        return entry.name
    }

    private var filterRow: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: Design.Space.m) {
                InkSearchField(placeholder: "Filter by name", query: $query, fill: Design.surface)
                    .frame(width: 200)
                FilterChip(label: "All", isOn: filter.isEmpty) {
                    filter = ModelFilter()
                }
                if !capabilityChips.isEmpty {
                    ChipDivider()
                    ForEach(capabilityChips) { chip in
                        FilterChip(
                            label: chip.label,
                            isOn: filter.capabilities.contains(chip.value),
                            count: chip.count,
                            isDisabled: chip.count == 0
                                && !filter.capabilities.contains(chip.value)
                        ) {
                            toggle(&filter.capabilities, chip.value)
                        }
                    }
                }
                if !sourceChips.isEmpty {
                    ChipDivider()
                    ForEach(sourceChips) { chip in
                        FilterChip(
                            label: chip.label,
                            isOn: filter.sources.contains(chip.value),
                            mark: chip.mark,
                            count: chip.count,
                            isDisabled: chip.count == 0 && !filter.sources.contains(chip.value)
                        ) {
                            toggle(&filter.sources, chip.value)
                        }
                    }
                }
                if !statusChips.isEmpty {
                    ChipDivider()
                    ForEach(statusChips) { chip in
                        FilterChip(
                            label: chip.label,
                            isOn: filter.statuses.contains(chip.value),
                            count: chip.count,
                            isDisabled: chip.count == 0 && !filter.statuses.contains(chip.value)
                        ) {
                            toggle(&filter.statuses, chip.value)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }

    private func adoptRequestedFilter() {
        guard let requested = shell.modelsFilter else { return }
        filter = requested
        query = ""
        shell.modelsFilter = nil
    }

    private func toggle<Value: Hashable>(_ set: inout Set<Value>, _ value: Value) {
        if set.contains(value) {
            set.remove(value)
        } else {
            set.insert(value)
        }
    }

    private var searched: [ModelRecord] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return shell.library.records }
        return shell.library.records.filter {
            $0.name.lowercased().contains(needle) || $0.displayName.lowercased().contains(needle)
        }
    }

    private func count(_ probe: ModelFilter) -> Int {
        searched.count { probe.matches($0, warm: isWarm($0)) }
    }

    private var capabilityChips: [ModelChip<AppMode>] {
        [AppMode.chat, .images, .voice].compactMap { mode in
            guard shell.library.records.contains(where: { ModelFilter.hasCapability($0, mode) })
            else { return nil }
            var probe = filter
            probe.capabilities = [mode]
            return ModelChip(value: mode, label: Design.modeTitle(mode), count: count(probe))
        }
    }

    private var sourceChips: [ModelChip<String>] {
        let order: [SourceKind] = [
            .ollama, .huggingfaceCache, .lmStudio, .builtin, .endpoint, .file, .folder,
        ]
        var titles: [String] = []
        for kind in order {
            let title = Self.storeTitle(kind)
            guard !titles.contains(title),
                shell.library.records.contains(where: {
                    Self.storeTitle($0.source.kind) == title
                })
            else { continue }
            titles.append(title)
        }
        for record in shell.library.records {
            let title = Self.storeTitle(record.source.kind)
            if !titles.contains(title) {
                titles.append(title)
            }
        }
        return titles.map { title in
            var probe = filter
            probe.sources = [title]
            let mark = shell.library.records.first {
                Self.storeTitle($0.source.kind) == title
            }?.source.kind
            return ModelChip(value: title, label: title, mark: mark, count: count(probe))
        }
    }

    private var statusChips: [ModelChip<ModelStatus>] {
        ModelStatus.allCases.compactMap { status in
            guard shell.library.records.contains(where: { status.matches($0, warm: isWarm($0)) })
            else { return nil }
            var probe = filter
            probe.statuses = [status]
            return ModelChip(value: status, label: status.label, count: count(probe))
        }
    }

    private func isWarm(_ record: ModelRecord) -> Bool {
        shell.isWarm(record)
    }

    private var filtered: [ModelRecord] {
        searched.filter { filter.matches($0, warm: isWarm($0)) }
    }

    @ViewBuilder
    private var gridContent: some View {
        if filtered.isEmpty {
            ModeEmptyState(
                glyph: "line.3.horizontal.decrease",
                eyebrow: "Filtered view",
                headline: "No models match.",
                caption: "Loosen the filter or search by another name."
            ) {
                if !filter.isEmpty || !query.isEmpty {
                    Button("Clear filters") {
                        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                            filter = ModelFilter()
                            query = ""
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                }
            }
            .frame(minHeight: 260)
        } else if sourceGroups.count <= 1 {
            modelGrid(filtered)
        } else {
            VStack(alignment: .leading, spacing: Design.Space.xl) {
                ForEach(sourceGroups, id: \.title) { group in
                    VStack(alignment: .leading, spacing: Design.Space.m) {
                        MicroHeader(title: "\(group.title) · \(group.records.count)")
                        modelGrid(group.records)
                    }
                }
            }
        }
    }

    private func modelGrid(_ records: [ModelRecord]) -> some View {
        LazyVGrid(
            columns: [
                GridItem(.adaptive(minimum: 280), spacing: Design.Space.l, alignment: .top)
            ],
            spacing: Design.Space.l
        ) {
            ForEach(records) { record in
                ModelCard(record: record, warm: isWarm(record)) {
                    presented = record.id
                    shell.selectLibrary(record.id)
                }
            }
        }
        .animation(
            Design.motion(reduceMotion: reduceMotion),
            value: records.map(\.id))
    }

    private var sourceGroups: [(title: String, records: [ModelRecord])] {
        var buckets: [String: [ModelRecord]] = [:]
        var order: [String] = []
        let canonical: [SourceKind] = [
            .ollama, .huggingfaceCache, .lmStudio, .builtin, .endpoint, .file, .folder,
        ]
        for kind in canonical {
            let title = Self.storeTitle(kind)
            if !order.contains(title) { order.append(title) }
        }
        for record in filtered {
            let title = Self.storeTitle(record.source.kind)
            buckets[title, default: []].append(record)
            if !order.contains(title) { order.append(title) }
        }
        return order.compactMap { title in
            guard let records = buckets[title], !records.isEmpty else { return nil }
            return (title: title, records: records)
        }
    }

    private var heroEmptyState: some View {
        VStack(spacing: 0) {
            Spacer()
            HedosLogo(size: 52, color: Design.inkSoft)
                .padding(.bottom, Design.Space.pane)
            if let failure = shell.library.errorMessage {
                Text("The scan hit a problem.")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                Text(failure)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .multilineTextAlignment(.center)
                    .lineSpacing(Design.bodyLineSpacing)
                    .frame(maxWidth: 420)
                    .padding(.top, Design.Space.s)
            } else if let summary = shell.library.summary {
                Text(summary.headline)
                    .font(Design.paneTitle)
                    .tracking(Design.tightTracking)
                    .multilineTextAlignment(.center)
                    .lineSpacing(5)
                    .frame(maxWidth: 430)
            } else {
                ShimmerText(
                    text: "Looking for models on this Mac…",
                    font: Design.paneTitle, tracked: false)
            }
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(Design.Space.pane)
    }

    nonisolated static func storeTitle(_ kind: SourceKind) -> String { kind.storeTitle }
}

extension SourceKind {
    var storeTitle: String {
        switch self {
        case .ollama: "Ollama"
        case .huggingfaceCache: "Hugging Face"
        case .lmStudio: "LM Studio"
        case .builtin: "Built in"
        case .endpoint: "Servers"
        case .file, .folder: "Loose"
        default: "Other"
        }
    }
}

struct ModelCard: View {
    let record: ModelRecord
    var warm = false
    let onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: Design.Space.l) {
                VStack(alignment: .leading, spacing: Design.Space.xxs) {
                    HStack(alignment: .center, spacing: Design.Space.l) {
                        SourceMark(kind: record.source.kind, size: 20)
                            .foregroundStyle(Design.inkSoft)
                            .frame(width: 24, height: 24)
                            .overlay(alignment: .topTrailing) {
                                if warm {
                                    AccentDot()
                                        .offset(x: 3, y: -3)
                                }
                            }
                        Text(record.name)
                            .font(Design.title)
                            .tracking(Design.tightTracking)
                            .lineLimit(1)
                            .truncationMode(.tail)
                        Spacer(minLength: 0)
                    }
                    Text(
                        "\(ModelsPane.storeTitle(record.source.kind)) · \(record.modality.rawValue)"
                    )
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.leading, 24 + Design.Space.l)
                }
                HStack(spacing: Design.Space.s) {
                    ForEach(capabilities, id: \.self) { mode in
                        TintChip(text: Design.modeTitle(mode), glyph: Design.modeGlyph(mode))
                    }
                    FitChip(record: record)
                    if record.state == .missing {
                        TintChip(text: "missing", glyph: "exclamationmark.triangle", faint: true)
                            .help("No longer found on disk")
                    }
                }
                Spacer(minLength: 0)
                HStack(alignment: .firstTextBaseline) {
                    Text(
                        record.footprintMB.map {
                            $0 > 0 ? DiscoverySummary.formatBytes(Int64($0) << 20) : "—"
                        } ?? "size unknown"
                    )
                    .font(Design.data(15))
                    .monospacedDigit()
                    .foregroundStyle(Design.ink)
                    Spacer()
                    Text(MetaGrid.tierWord(record.runtime.tier).uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, minHeight: 132, alignment: .topLeading)
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
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help("Show details")
        .accessibilityLabel(record.displayName)
        .accessibilityIdentifier("model-card-\(record.id)")
    }

    private var capabilities: [AppMode] {
        [AppMode.chat, .images, .voice].filter {
            !Launcher.models(in: [record], for: $0).isEmpty
        }
    }
}

struct BestFitCard: View {
    let record: ModelRecord
    let onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                MicroHeader(title: "Best fit")
                HStack(alignment: .top, spacing: Design.Space.l) {
                    SourceMark(kind: record.source.kind, size: 18)
                        .foregroundStyle(Design.inkSoft)
                        .frame(width: 20, height: 20)
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        Text("\(record.displayName) fits this Mac best.")
                            .font(Design.title)
                            .tracking(Design.tightTracking)
                            .foregroundStyle(Design.ink)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(subtitle)
                            .font(Design.data(11))
                            .foregroundStyle(Design.inkFaint)
                    }
                    Spacer(minLength: 0)
                }
                Spacer(minLength: 0)
                HStack(spacing: Design.Space.s) {
                    TintChip(
                        text: Design.modeTitle(destination),
                        glyph: Design.modeGlyph(destination))
                    FitChip(record: record)
                }
            }
            .padding(Design.Space.xl)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(Design.Radius.card))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help("Show details")
        .accessibilityLabel("\(record.displayName) fits this Mac best")
        .accessibilityIdentifier("models-best-fit")
    }

    private var destination: AppMode {
        Launcher.destination(for: record)
    }

    private var subtitle: String {
        var parts: [String] = []
        if let mb = record.footprintMB, mb > 0 {
            parts.append(DiscoverySummary.formatBytes(Int64(mb) << 20))
        }
        if let runtime = record.runtime.id {
            parts.append(runtime.rawValue)
        }
        parts.append("ready")
        return parts.joined(separator: " · ")
    }
}

struct ModelDetailSheet: View {
    let record: ModelRecord
    let shell: ShellModel
    let onClose: () -> Void
    @State private var reason: String?
    @State private var consent: ManifestConsentInfo?
    @State private var approvedConsent: ManifestConsentInfo?
    @State private var communityRecipes: [RuntimeInstallPreview] = []
    @State private var installingRecipe: String?
    @State private var chosenRuntime: String

    init(record: ModelRecord, shell: ShellModel, onClose: @escaping () -> Void) {
        self.record = record
        self.shell = shell
        self.onClose = onClose
        _chosenRuntime = State(initialValue: record.runtime.id?.rawValue ?? "")
    }

    private var currentRuntimeValue: String {
        record.runtime.id?.rawValue ?? ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            SheetDivider()
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.gutter) {
                    if record.runtime.tier == .recipeNeeded {
                        Text(reason ?? "")
                            .font(Design.body)
                            .foregroundStyle(Design.inkSoft)
                            .lineSpacing(Design.bodyLineSpacing)
                            .frame(maxWidth: Design.Column.prose, alignment: .leading)
                    }
                    specs
                    if showsRuntimeChoice {
                        runtimeSection
                    }
                    if record.runtime.tier != .recipeNeeded {
                        ModelConfigureSection(record: record, shell: shell)
                    }
                    if let approvedConsent {
                        revokeRow(approvedConsent)
                    }
                }
                .sheetBodyPadding()
            }
            SheetDivider()
            footer
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
        }
        .frame(width: Design.Sheet.modelDetailWidth, height: record.runtime.tier == .recipeNeeded ? Design.Sheet.modelRecipeHeight : Design.Sheet.modelDetailHeight)
        .task(id: record.id) {
            guard record.runtime.tier == .recipeNeeded else { return }
            let record = record
            reason = await Task.detached {
                let identified = Identification.identify(record)
                return RecipeReason.text(
                    for: record, format: identified.format,
                    pipelineClass: identified.pipelineClass)
            }.value
            consent = try? await shell.kernel.pendingHostConsent(for: record.id)
            approvedConsent = try? await shell.kernel.approvedHostConsent(for: record.id)
            communityRecipes = await shell.kernel.communityRecipes(for: record.id)
        }
        .onChange(of: currentRuntimeValue) { _, value in
            chosenRuntime = value
        }
    }

    private var header: some View {
        SheetHeader(
            title: record.displayName,
            onClose: onClose,
            plaque: {
                SourceMark(kind: record.source.kind, size: 24)
                    .foregroundStyle(Design.inkSoft)
            },
            below: {
                HStack(spacing: Design.Space.s) {
                    TintChip(
                        text: record.modality.rawValue,
                        glyph: Design.modalityGlyph(record.modality))
                    FitChip(record: record)
                    TintChip(text: MetaGrid.tierWord(record.runtime.tier))
                    if let runtime = record.runtime.id {
                        TintChip(text: runtime.rawValue)
                    }
                }
            })
    }

    private var runtimeSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Runtime")
            InkRadioGroup(options: runtimeOptions, selection: $chosenRuntime)
                .padding(.horizontal, Design.Space.s)
                .padding(.vertical, Design.Space.s)
                .surfaceCard(radius: Design.Radius.card)
            Text(needsConfirmation
                ? "Nothing runs until you open it."
                : "Switching takes effect next time you open it.")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
        }
    }

    private var runtimeOptions: [(value: String, label: String)] {
        var options = [(value: currentRuntimeValue, label: runtimeLabel(currentRuntimeValue))]
        for alt in record.runtime.alternatives {
            options.append((value: alt.rawValue, label: runtimeLabel(alt.rawValue)))
        }
        return options
    }

    private var specs: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Specification")
            specCard
            if let group = Fit.duplicateInsight(record, in: shell.library.summary) {
                duplicateCard(group)
            }
        }
    }

    private var scalarSpecs: [(String, String, Bool)] {
        var items: [(String, String, Bool)] = [("Kind", record.source.kind.rawValue, false)]
        if let repo = record.source.repo {
            items.append(("Repo", repo, false))
        }
        if let mb = record.footprintMB, mb > 0 {
            items.append(("On disk", DiscoverySummary.formatBytes(Int64(mb) << 20), true))
        }
        if let path = record.primaryWeightPath ?? record.source.path as String? {
            items.append(("Path", (path as NSString).abbreviatingWithTildeInPath, false))
        }
        return items
    }

    private var specCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            if record.displayName != record.name {
                sourceRow
                if !scalarSpecs.isEmpty { DottedRule() }
            }
            ForEach(Array(scalarSpecs.enumerated()), id: \.offset) { index, item in
                specRow(item.0, item.1, mono: item.2)
                if index < scalarSpecs.count - 1 { DottedRule() }
            }
        }
        .padding(.horizontal, Design.Space.tile)
        .padding(.vertical, Design.Space.xs)
        .surfaceCard(radius: Design.Radius.card)
    }

    private var sourceRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: Design.Space.l) {
            Text("Source")
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .fixedSize()
                .layoutPriority(1)
            Spacer(minLength: Design.Space.m)
            SourceMark(kind: record.source.kind, size: 12)
                .foregroundStyle(Design.inkFaint)
            Text(record.name)
                .font(Design.data(12))
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .padding(.vertical, Design.Space.m)
    }

    private func duplicateCard(_ group: DuplicateGroup) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            MicroHeader(title: "Shared weights")
            Text(
                "The same weights also live as \(group.names.filter { $0 != record.displayName && $0 != record.name }.joined(separator: ", ")), \(DiscoverySummary.formatBytes(group.wastedBytes)) of disk counted twice. Hedos points at both; nothing is copied."
            )
            .font(Design.caption)
            .foregroundStyle(Design.inkSoft)
            .lineSpacing(Design.bodyLineSpacing)
            .fixedSize(horizontal: false, vertical: true)
            if let note = configuredSiblingNote(group) {
                Text(note)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .lineSpacing(Design.bodyLineSpacing)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(Design.Space.tile)
        .surfaceCard(radius: Design.Radius.card)
        .accessibilityIdentifier("duplicate-insight")
    }

    private func configuredSiblingNote(_ group: DuplicateGroup) -> String? {
        guard !Self.hasUserConfig(record) else { return nil }
        let siblings = shell.library.records.filter { candidate in
            candidate.id != record.id
                && (candidate.primaryWeightPath.map(group.paths.contains) ?? false)
        }
        let configured = siblings.filter(Self.hasUserConfig)
        guard configured.count == 1, let one = configured.first else { return nil }
        return "A configured copy exists: \(one.displayName). Its settings stay with that copy."
    }

    private static func hasUserConfig(_ record: ModelRecord) -> Bool {
        !record.paramValues.isEmpty || record.systemPrompt?.isEmpty == false
            || record.alias?.isEmpty == false
    }

    private func specRow(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Design.Space.l) {
            Text(label)
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .fixedSize()
                .layoutPriority(1)
            Text(value)
                .font(Design.data(12))
                .monospacedDigit()
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .padding(.vertical, Design.Space.m)
    }

    @ViewBuilder
    private func revokeRow(_ consent: ManifestConsentInfo) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            MicroHeader(title: "Access granted")
            Text(
                consent.network
                    ? "\(consent.id) runs on this Mac with network access."
                    : "\(consent.id) runs its code on this Mac, sandboxed."
            )
            .font(Design.label)
            .foregroundStyle(Design.inkSoft)
            Button("Revoke \(consent.id)") {
                let shell = shell
                let id = consent.id
                Task {
                    try? await shell.kernel.revokeNetworkRuntime(id)
                    await shell.library.refreshShelf()
                }
            }
            .buttonStyle(InkButtonStyle())
            .padding(.top, Design.Space.xs)
        }
    }

    @ViewBuilder
    private var footer: some View {
        if record.runtime.tier == .recipeNeeded {
            VStack(alignment: .leading, spacing: Design.Space.l) {
                if let consent {
                    consentCard(consent)
                } else if !communityRecipes.isEmpty {
                    ForEach(communityRecipes, id: \.id) { recipe in
                        recipeCard(recipe)
                    }
                } else {
                    Text("A runtime recipe can make this model runnable later.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
                HStack(spacing: Design.Space.m) {
                    ConfirmingButton(
                        label: "Copy manifest template", confirmedLabel: "Copied",
                        appearance: .plain
                    ) {
                        let template = ManifestTemplate.template(for: record)
                        let pasteboard = NSPasteboard.general
                        pasteboard.clearContents()
                        pasteboard.setString(template, forType: .string)
                    }
                    Button("Open runtimes.d…") {
                        NSWorkspace.shared.activateFileViewerSelecting(
                            [shell.kernel.runtimeCatalog.ensuredDirectory()])
                    }
                    .buttonStyle(QuietButtonStyle())
                    Spacer(minLength: 0)
                }
            }
        } else if let title = openTitle {
            VStack(spacing: Design.Space.m) {
                Button {
                    confirmAndOpen()
                } label: {
                    Text(title)
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                if isChatModel {
                    Button("Make this the default chat model") {
                        Task {
                            try? await shell.kernel.settings.setDefaultChatModelID(record.id)
                        }
                    }
                    .buttonStyle(.plain)
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
                }
            }
        }
    }

    private func consentCard(_ consent: ManifestConsentInfo) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Text("Runs its code on this Mac, sandboxed")
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.ink)
            grantList(network: consent.network, paths: consent.paths)
            Button("Approve \(consent.id)") {
                let shell = shell
                let id = consent.id
                Task {
                    try? await shell.kernel.approveNetworkRuntime(id)
                    await shell.library.refreshShelf()
                }
            }
            .buttonStyle(InkButtonStyle())
            .padding(.top, Design.Space.xs)
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.card)
    }

    private func recipeCard(_ recipe: RuntimeInstallPreview) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack(alignment: .firstTextBaseline, spacing: Design.Space.m) {
                Text(recipe.id)
                    .font(Design.caption.weight(.semibold))
                    .foregroundStyle(Design.ink)
                Spacer(minLength: Design.Space.m)
                Text("Community recipe")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            if !recipe.capabilities.isEmpty {
                Text(recipe.capabilities.joined(separator: ", "))
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
            }
            grantList(network: false, paths: recipe.paths)
            Button {
                let shell = shell
                let source = recipe.sourceURL
                let id = recipe.id
                installingRecipe = id
                Task {
                    _ = try? await shell.kernel.installRuntime(from: source)
                    await shell.library.refreshShelf()
                    installingRecipe = nil
                }
            } label: {
                Text(installingRecipe == recipe.id ? "Installing…" : "Install \(recipe.id)")
                    .contentTransition(.opacity)
            }
            .buttonStyle(InkButtonStyle())
            .disabled(installingRecipe != nil)
            .animation(Design.wash, value: installingRecipe)
            .padding(.top, Design.Space.xs)
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.card)
    }

    @ViewBuilder
    private func grantList(network: Bool, paths: [String]) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.xxs) {
            if network {
                grantRow(glyph: "network", text: "Network access — outbound connections allowed")
            }
            ForEach(paths, id: \.self) { path in
                grantRow(glyph: "folder", text: path)
            }
        }
    }

    private func grantRow(glyph: String, text: String) -> some View {
        HStack(spacing: Design.Space.s) {
            Image(systemName: glyph)
                .font(Design.glyphSmall)
                .foregroundStyle(Design.inkFaint)
            Text(text)
                .font(Design.label)
                .foregroundStyle(Design.inkSoft)
                .lineLimit(1)
                .truncationMode(.middle)
        }
    }

    private var isChatModel: Bool {
        Launcher.destination(for: record) == .chat
    }

    private var needsConfirmation: Bool {
        record.runtime.resolved == .auto && record.runtime.confirmedAt == nil
            && record.runtime.tier != .recipeNeeded
    }

    private var showsRuntimeChoice: Bool {
        guard record.runtime.tier != .recipeNeeded else { return false }
        return needsConfirmation || !record.runtime.alternatives.isEmpty
    }

    private var openTitle: String? {
        switch Launcher.destination(for: record) {
        case .chat: "Open in Chat"
        case .images: "Open in Images"
        case .voice: "Open in Voice"
        default: nil
        }
    }

    private func runtimeLabel(_ id: String) -> String {
        id == record.runtime.id?.rawValue ? "\(id) · suggested" : id
    }

    private func confirmAndOpen() {
        let record = record
        let shell = shell
        let chosen = chosenRuntime
        let confirming = needsConfirmation
        onClose()
        Task {
            if confirming {
                if chosen == currentRuntimeValue {
                    await shell.library.confirmRuntime(record.id)
                } else {
                    await shell.library.overrideRuntime(record.id, to: chosen)
                }
            } else if chosen != currentRuntimeValue {
                await shell.library.overrideRuntime(record.id, to: chosen)
            }
            if let fresh = shell.library.record(id: record.id) {
                shell.launch(fresh)
            }
        }
    }
}

enum RecipeReason {
    nonisolated static func text(
        for record: ModelRecord, format: ModelFormat, pipelineClass: String?
    ) -> String {
        switch format {
        case .unknown:
            return record.primaryWeightPath == nil
                ? "This model's weights are in a format none of the built-in runtimes can execute. No safetensors or GGUF detected, likely PyTorch or another framework's format."
                : "Hedos found this model but could not identify what kind it is."
        case .diffusers:
            if let pipelineClass {
                return "This is a diffusers \(pipelineClass) bundle; no built-in runtime serves it yet."
            }
            return "This is an image-generation pipeline the built-in image runtime cannot serve yet."
        case .safetensors, .mlxSafetensors:
            return "The format is recognized, but no built-in runtime serves \(record.modality.rawValue) models yet."
        default:
            return "No built-in runtime can execute this model yet."
        }
    }
}

struct ModelConfigureSection: View {
    let record: ModelRecord
    let shell: ShellModel
    @State private var aliasDraft = ""
    @State private var promptDraft = ""
    @State private var seeded = false
    @State private var promptCommit: Task<Void, Never>?
    @State private var voices: [String] = []
    @State private var pending: [String: JSONValue?] = [:]
    @State private var flush: Task<Void, Never>?

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Configure")
            configureRows
                .padding(.horizontal, Design.Space.tile)
                .padding(.vertical, Design.Space.xs)
                .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private var configureRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            row("Display name") {
                InkField(
                    placeholder: record.name,
                    text: $aliasDraft,
                    size: .settings,
                    onSubmit: { commitAlias() },
                    onFocusLost: { commitAlias() }
                )
                .frame(width: Design.Control.fieldWidth)
                .accessibilityLabel("Display name")
            }
            if record.capabilities.contains(.chat) {
                Divider()
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    HStack(spacing: Design.Space.s) {
                        Text("System prompt")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                        Text("· prepended to every conversation")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint.opacity(0.7))
                    }
                    InkTextArea(placeholder: "Optional", text: $promptDraft, resizable: true)
                        .accessibilityLabel("System prompt")
                }
                .padding(.vertical, Design.Space.m)
            }
            ForEach(record.params, id: \.key) { spec in
                Divider()
                parameterRow(spec)
            }
            if !record.params.isEmpty {
                Divider()
                HStack {
                    Text("Overrides apply to the next generation.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    Spacer()
                    Button("Reset to model defaults") {
                        let shell = shell
                        let id = record.id
                        Task {
                            try? await shell.kernel.resetParamValues(id)
                            await shell.library.refreshShelf()
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(record.paramValues.isEmpty)
                }
                .padding(.vertical, Design.Space.m)
            }
            Text("Auto means the model decides. Nothing is sent unless you set it.")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .padding(.top, Design.Space.m)
        }
        .onAppear { seedDrafts() }
        .task(id: record.id) {
            guard record.capabilities.contains(.speak) else { return }
            voices = (try? await shell.kernel.voices(for: record.id)) ?? []
        }
        .onChange(of: record.id) {
            seeded = false
            seedDrafts()
        }
        .onChange(of: promptDraft) {
            guard seeded else { return }
            promptCommit?.cancel()
            let shell = shell
            let id = record.id
            let draft = promptDraft.trimmingCharacters(in: .whitespacesAndNewlines)
            guard draft != (record.systemPrompt ?? "") else { return }
            promptCommit = Task {
                try? await Task.sleep(for: .milliseconds(700))
                guard !Task.isCancelled else { return }
                try? await shell.kernel.setSystemPrompt(id, to: draft.isEmpty ? nil : draft)
                await shell.library.refreshShelf()
            }
        }
    }

    private func row(
        _ label: String, @ViewBuilder control: () -> some View
    ) -> some View {
        HStack(alignment: .center) {
            Text(label)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
            Spacer(minLength: Design.Space.l)
            control()
        }
        .padding(.vertical, Design.Space.m)
    }

    @ViewBuilder
    private func parameterRow(_ spec: ParamSpec) -> some View {
        if spec.type == .enumeration {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                parameterLabel(spec)
                parameterControl(spec)
            }
            .padding(.vertical, Design.Space.chipX)
        } else {
            HStack(alignment: .center, spacing: Design.Space.s) {
                parameterLabel(spec)
                Spacer(minLength: Design.Space.l)
                parameterControl(spec)
                    .frame(width: 190, alignment: .trailing)
            }
            .padding(.vertical, Design.Space.chipX)
        }
    }

    private func parameterLabel(_ spec: ParamSpec) -> some View {
        HStack(spacing: Design.Space.s) {
            Text(humanized(spec.key))
                .font(Design.caption)
                .foregroundStyle(effective(spec.key) != nil ? Design.ink : Design.inkSoft)
            if effective(spec.key) != nil {
                Circle()
                    .fill(Design.ink)
                    .frame(width: 5, height: 5)
                    .accessibilityLabel("Overridden")
                Button {
                    write(spec.key, nil)
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(Design.glyphSmall)
                        .foregroundStyle(Design.inkFaint)
                }
                .buttonStyle(.plain)
                .help("Clear the override")
                .accessibilityLabel("Clear \(spec.key) override")
            }
        }
    }

    private func humanized(_ key: String) -> String {
        key.split(separator: "_")
            .map { $0.prefix(1).uppercased() + $0.dropFirst() }
            .joined(separator: " ")
    }

    @ViewBuilder
    private func parameterControl(_ spec: ParamSpec) -> some View {
        if spec.key == "voice", !voices.isEmpty {
            InkDropdown(
                options: voices,
                selection: storedVoice,
                accessibilityName: "voice",
                onSelect: { choice in
                    write("voice", choice.map(JSONValue.string))
                })
        } else {
            ParamControl(
                spec: spec,
                get: { effective(spec.key) },
                set: { value in write(spec.key, value) })
        }
    }

    private func effective(_ key: String) -> JSONValue? {
        if let queued = pending[key] {
            return queued
        }
        return record.paramValues[key]
    }

    private var storedVoice: String? {
        if case .string(let value)? = effective("voice") {
            return value
        }
        return nil
    }

    private func write(_ key: String, _ value: JSONValue?) {
        let spec = record.params.first { $0.key == key }
        let normalized = value == spec?.defaultValue ? nil : value
        pending[key] = normalized
        scheduleFlush()
    }

    private func scheduleFlush() {
        flush?.cancel()
        let shell = shell
        let id = record.id
        flush = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            let batch = pending
            for (key, value) in batch {
                try? await shell.kernel.setParamValue(id, key: key, to: value)
            }
            await shell.library.refreshShelf()
            pending = pending.filter { entry in
                guard let flushed = batch[entry.key] else { return true }
                return flushed != entry.value
            }
            if !pending.isEmpty {
                scheduleFlush()
            }
        }
    }

    private func commitAlias() {
        let trimmed = aliasDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != (record.alias ?? "") else { return }
        let shell = shell
        let id = record.id
        Task {
            try? await shell.kernel.setAlias(id, to: trimmed.isEmpty ? nil : trimmed)
            await shell.library.refreshShelf()
        }
    }

    private func seedDrafts() {
        guard !seeded else { return }
        aliasDraft = record.alias ?? ""
        promptDraft = record.systemPrompt ?? ""
        seeded = true
    }
}
