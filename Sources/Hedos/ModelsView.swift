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
    @State private var filter = ModelFilter()
    @State private var query = ""
    @State private var showFolders = false
    @State private var presented: String?

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Models") {
                if shell.library.isScanning {
                    ProgressView()
                        .controlSize(.small)
                }
                QuietIconButton(glyph: "folder.badge.plus") {
                    showFolders.toggle()
                }
                .help("Watched folders")
                .accessibilityLabel("Watched folders")
                .inkPopover(
                    isPresented: $showFolders,
                    width: Design.Popover.menuWidth,
                    maxHeight: Design.Popover.menuMaxHeight
                ) {
                    FoldersPopover(model: shell.library) {
                        showFolders = false
                        shell.settingsTarget = SettingsDestination(
                            section: .models, anchor: "models.folders")
                        SettingsWindowController.shared.show(shell: shell)
                    }
                }
                QuietIconButton(glyph: "arrow.clockwise") {
                    Task { await shell.library.rescan() }
                }
                .disabled(shell.library.isScanning)
                .help("Scan the machine again")
                .accessibilityLabel("Rescan")
            }
            filterRow
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            grid
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
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
            .padding(.horizontal, Design.Space.gutter)
            .padding(.bottom, Design.Space.l)
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
        shell.resident.contains { $0.modelID == record.id || $0.name == record.name }
    }

    private var filtered: [ModelRecord] {
        searched.filter { filter.matches($0, warm: isWarm($0)) }
    }

    @ViewBuilder
    private var grid: some View {
        if shell.library.records.isEmpty {
            heroEmptyState
        } else if filtered.isEmpty {
            ModeEmptyState(
                eyebrow: "Filtered view",
                headline: "No models match.",
                caption: "Loosen the filter or search by another name."
            ) {
                if !filter.isEmpty || !query.isEmpty {
                    Button("Clear filters") {
                        filter = ModelFilter()
                        query = ""
                    }
                    .buttonStyle(QuietButtonStyle())
                }
            }
        } else {
            ScrollView {
                LazyVGrid(
                    columns: [
                        GridItem(.adaptive(minimum: 280), spacing: Design.Space.l, alignment: .top)
                    ],
                    spacing: Design.Space.xxl
                ) {
                    ForEach(filtered) { record in
                        ModelCard(record: record, warm: isWarm(record)) {
                            presented = record.id
                            shell.selectLibrary(record.id)
                        }
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.xl)
                .padding(.bottom, Design.Space.gutter)
                .animation(
                    Design.motion(
                        reduceMotion: NSWorkspace.shared
                            .accessibilityDisplayShouldReduceMotion),
                    value: filtered.map(\.id))
            }
        }
    }

    private var heroEmptyState: some View {
        VStack(spacing: 0) {
            Spacer()
            HeptagonMark(size: 52, color: Design.ink)
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
                Text("Looking for models on this Mac…")
                    .font(Design.paneTitle)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.inkSoft)
                ProgressView()
                    .controlSize(.small)
                    .padding(.top, Design.Space.xl)
            }
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(Design.Space.pane)
    }

    static func storeTitle(_ kind: SourceKind) -> String {
        switch kind {
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

struct ModelDetailSheet: View {
    let record: ModelRecord
    let shell: ShellModel
    let onClose: () -> Void
    @State private var reason: String?
    @State private var consent: ManifestConsentInfo?
    @State private var copiedTemplate = false
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
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    if record.runtime.tier == .recipeNeeded {
                        Text(reason ?? "")
                            .font(Design.body)
                            .foregroundStyle(Design.inkSoft)
                            .lineSpacing(Design.bodyLineSpacing)
                            .frame(maxWidth: Design.Column.prose, alignment: .leading)
                    }
                    specs
                    if showsRuntimeChoice {
                        VStack(alignment: .leading, spacing: Design.Space.m) {
                            MicroHeader(title: "[ Runtime ]")
                            InkRadioGroup(
                                options: runtimeOptions, selection: $chosenRuntime)
                            Text(needsConfirmation
                                ? "Nothing runs until you open it."
                                : "Switching takes effect next time you open it.")
                                .font(Design.label)
                                .foregroundStyle(Design.inkFaint)
                        }
                    }
                    if record.runtime.tier != .recipeNeeded {
                        ModelConfigureSection(record: record, shell: shell)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
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
            consent = try? await shell.kernel.pendingNetworkConsent(for: record.id)
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                SourceMark(kind: record.source.kind, size: 24)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.s) {
                Text(record.displayName)
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .lineLimit(1)
                HStack(spacing: Design.Space.s) {
                    FitChip(record: record)
                    TintChip(text: MetaGrid.tierWord(record.runtime.tier))
                    if let runtime = record.runtime.id {
                        TintChip(text: runtime.rawValue)
                    }
                }
            }
            Spacer()
            SheetCloseButton(action: onClose)
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
            MicroHeader(title: "[ Specification ]")
            specRows
                .padding(.horizontal, Design.Space.tile)
                .padding(.vertical, Design.Space.s)
                .surfaceCard(radius: Design.Radius.card)
        }
    }

    private var specRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            if record.displayName != record.name {
                HStack(alignment: .firstTextBaseline, spacing: Design.Space.l) {
                    Text("Source")
                        .font(Design.caption)
                        .foregroundStyle(Design.inkFaint)
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
                .overlay(alignment: .bottom) { DottedRule() }
            }
            specRow("Modality", record.modality.rawValue)
            specRow("Kind", record.source.kind.rawValue)
            if let repo = record.source.repo {
                specRow("Repo", repo)
            }
            if let mb = record.footprintMB, mb > 0 {
                specRow("On disk", DiscoverySummary.formatBytes(Int64(mb) << 20), mono: true)
            }
            if let fit = Fit.label(record) {
                specRow("Fit", fit)
            }
            if let path = record.primaryWeightPath ?? record.source.path as String? {
                specRow("Path", (path as NSString).abbreviatingWithTildeInPath)
            }
            if let group = Fit.duplicateInsight(record, in: shell.library.summary) {
                duplicateCard(group)
            }
        }
    }

    private func duplicateCard(_ group: DuplicateGroup) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            Text("Shared weights".uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
            Text(
                "The same weights also live as \(group.names.filter { $0 != record.displayName && $0 != record.name }.joined(separator: ", ")), \(DiscoverySummary.formatBytes(group.wastedBytes)) of disk counted twice. Hedos points at both; nothing is copied."
            )
            .font(Design.caption)
            .foregroundStyle(Design.inkSoft)
            .lineSpacing(Design.bodyLineSpacing)
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, Design.Space.m)
        .accessibilityIdentifier("duplicate-insight")
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
        .overlay(alignment: .bottom) { DottedRule() }
    }

    @ViewBuilder
    private var footer: some View {
        if record.runtime.tier == .recipeNeeded {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                if let consent {
                    VStack(alignment: .leading, spacing: Design.Space.xs) {
                        Text("A runtime for this model exists but wants permissions:")
                            .font(Design.label)
                            .foregroundStyle(Design.inkSoft)
                        Text("Network access — outbound connections allowed")
                            .font(Design.label)
                            .foregroundStyle(Design.ink)
                        ForEach(consent.paths, id: \.self) { path in
                            Text("Files — \(path)")
                                .font(Design.label)
                                .foregroundStyle(Design.ink)
                        }
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
                } else {
                    Text("A runtime recipe can make this model runnable later.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
                HStack(spacing: Design.Space.m) {
                    Button(copiedTemplate ? "Copied" : "Copy manifest template") {
                        let shell = shell
                        let id = record.id
                        Task { @MainActor in
                            guard
                                let template = try? await shell.kernel.manifestTemplate(for: id)
                            else { return }
                            let pasteboard = NSPasteboard.general
                            pasteboard.clearContents()
                            pasteboard.setString(template, forType: .string)
                            copiedTemplate = true
                            try? await Task.sleep(for: .seconds(2))
                            copiedTemplate = false
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                    Button("Open runtimes.d…") {
                        NSWorkspace.shared.activateFileViewerSelecting(
                            [shell.kernel.userRuntimesDirectory()])
                    }
                    .buttonStyle(QuietButtonStyle())
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
                            try? await shell.kernel.setDefaultChatModel(record.id)
                        }
                    }
                    .buttonStyle(.plain)
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
                }
            }
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
                    onSubmit: { commitAlias() },
                    onFocusLost: { commitAlias() }
                )
                .frame(width: 200)
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
            voices = (try? await shell.kernel.voices(record.id)) ?? []
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
            pending = pending.filter { !batch.keys.contains($0.key) }
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
