import AppKit
import HedosKernel
import SwiftUI

enum ModelFacet: Hashable {
    case all
    case capability(AppMode)
    case store(SourceKind)
    case recipeNeeded

    var label: String {
        switch self {
        case .all: "All"
        case .capability(let mode): Design.modeTitle(mode)
        case .store(let kind): ModelsPane.storeTitle(kind)
        case .recipeNeeded: "Needs recipe"
        }
    }

    var storeKind: SourceKind? {
        if case .store(let kind) = self { return kind }
        return nil
    }

    func matches(_ record: ModelRecord) -> Bool {
        switch self {
        case .all:
            true
        case .capability(let mode):
            Launcher.models(in: [record], for: mode).isEmpty == false
        case .store(let kind):
            ModelsPane.storeTitle(record.source.kind) == ModelsPane.storeTitle(kind)
        case .recipeNeeded:
            record.runtime.tier == .recipeNeeded
        }
    }
}

struct ModelsPane: View {
    @Bindable var shell: ShellModel
    @State private var facet: ModelFacet = .all
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
                .popover(isPresented: $showFolders, arrowEdge: .bottom) {
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
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "magnifyingglass")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkFaint)
                    InkField(placeholder: "Filter by name", text: $query, shape: .capsule)
                        .frame(width: 180)
                }
                ForEach(facets, id: \.self) { candidate in
                    FilterChip(
                        label: candidate.label,
                        isOn: facet == candidate,
                        mark: candidate.storeKind
                    ) {
                        facet = facet == candidate ? .all : candidate
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.bottom, Design.Space.l)
        }
    }

    private var facets: [ModelFacet] {
        var list: [ModelFacet] = [
            .all, .capability(.chat), .capability(.images), .capability(.voice),
        ]
        let kinds: [SourceKind] = [
            .ollama, .huggingfaceCache, .lmStudio, .builtin, .endpoint, .file, .folder,
        ]
        for kind in kinds
        where shell.library.records.contains(where: { $0.source.kind == kind }) {
            if case .store(let existing) = list.last, Self.storeTitle(existing) == Self.storeTitle(kind) {
                continue
            }
            list.append(.store(kind))
        }
        if shell.library.records.contains(where: { $0.runtime.tier == .recipeNeeded }) {
            list.append(.recipeNeeded)
        }
        return list
    }

    private func isWarm(_ record: ModelRecord) -> Bool {
        shell.resident.contains { $0.modelID == record.id || $0.name == record.name }
    }

    private var filtered: [ModelRecord] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        return shell.library.records.filter { record in
            facet.matches(record)
                && (needle.isEmpty || record.name.lowercased().contains(needle)
                    || record.displayName.lowercased().contains(needle))
        }
    }

    @ViewBuilder
    private var grid: some View {
        if shell.library.records.isEmpty {
            heroEmptyState
        } else if filtered.isEmpty {
            ModeEmptyState(
                eyebrow: "Filtered view",
                headline: "No models match.",
                caption: "Loosen the filter or search by another name.")
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
                HStack(alignment: .top, spacing: Design.Space.l) {
                    IconPlaque(size: 44) {
                        SourceMark(kind: record.source.kind, size: 24)
                            .foregroundStyle(Design.inkSoft)
                    }
                    .overlay(alignment: .topTrailing) {
                        if warm {
                            AccentDot()
                                .offset(x: 2, y: -2)
                        }
                    }
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        Text(record.name)
                            .font(Design.title)
                            .tracking(Design.tightTracking)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                        Text(
                            "\(ModelsPane.storeTitle(record.source.kind)) · \(record.modality.rawValue)"
                        )
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    }
                    Spacer(minLength: 0)
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
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
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
        _chosenRuntime = State(initialValue: record.runtime.id ?? "")
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
                    if needsConfirmation {
                        VStack(alignment: .leading, spacing: Design.Space.m) {
                            MicroHeader(title: "Runtime")
                            InkRadioGroup(
                                options: runtimeOptions, selection: $chosenRuntime)
                            Text("Nothing runs until you open it.")
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
                        TintChip(text: runtime)
                    }
                }
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var runtimeOptions: [(value: String, label: String)] {
        var options = [(value: record.runtime.id ?? "", label: runtimeLabel(record.runtime.id ?? ""))]
        for alt in record.runtime.alternatives {
            options.append((value: alt, label: runtimeLabel(alt)))
        }
        return options
    }

    private var specs: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Details")
            specRows
                .padding(.horizontal, Design.Space.tile)
                .padding(.vertical, Design.Space.xs)
                .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private var specRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            if record.displayName != record.name {
                HStack(alignment: .firstTextBaseline) {
                    Text("Source")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .frame(width: 72, alignment: .leading)
                    SourceMark(kind: record.source.kind, size: 12)
                        .foregroundStyle(Design.inkFaint)
                    Text(record.name)
                        .font(Design.caption)
                        .foregroundStyle(Design.ink)
                        .textSelection(.enabled)
                        .lineLimit(2)
                        .truncationMode(.middle)
                    Spacer(minLength: 0)
                }
                .padding(.vertical, Design.Space.m)
                Divider()
            }
            specRow("Modality", record.modality.rawValue)
            Divider()
            specRow("Kind", record.source.kind.rawValue)
            if let repo = record.source.repo {
                Divider()
                specRow("Repo", repo)
            }
            if let mb = record.footprintMB, mb > 0 {
                Divider()
                specRow("Size", DiscoverySummary.formatBytes(Int64(mb) << 20), mono: true)
            }
            if let fit = Fit.label(record) {
                Divider()
                specRow("Fit", fit)
            }
            if let path = record.primaryWeightPath ?? record.source.path as String? {
                Divider()
                specRow("Path", (path as NSString).abbreviatingWithTildeInPath)
            }
            if let group = Fit.duplicateInsight(record, in: shell.library.summary) {
                Divider()
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
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .frame(width: 72, alignment: .leading)
            Text(value)
                .font(mono ? Design.data(12) : Design.caption)
                .monospacedDigit()
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.vertical, Design.Space.m)
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

    private var openTitle: String? {
        switch Launcher.destination(for: record) {
        case .chat: "Open in Chat"
        case .images: "Open in Images"
        case .voice: "Open in Voice"
        default: nil
        }
    }

    private func runtimeLabel(_ id: String) -> String {
        id == record.runtime.id ? "\(id) · suggested" : id
    }

    private func confirmAndOpen() {
        let record = record
        let shell = shell
        let chosen = chosenRuntime
        let confirming = needsConfirmation
        onClose()
        Task {
            if confirming {
                if chosen == record.runtime.id ?? "" {
                    await shell.library.confirmRuntime(record.id)
                } else {
                    await shell.library.overrideRuntime(record.id, to: chosen)
                }
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
