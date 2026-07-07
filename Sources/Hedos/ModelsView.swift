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

    func matches(_ record: ModelRecord) -> Bool {
        switch self {
        case .all:
            true
        case .capability(let mode):
            Launcher.models(in: [record], for: mode).isEmpty == false
        case .store(let kind):
            record.source.kind == kind
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
            filterRow
                .padding(.top, Design.Space.l)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            grid
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .navigationTitle("Models")
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                if shell.library.isScanning {
                    ProgressView()
                        .controlSize(.small)
                }
                Button {
                    showFolders.toggle()
                } label: {
                    Image(systemName: "folder.badge.plus")
                }
                .help("Watched folders")
                .accessibilityLabel("Watched folders")
                .popover(isPresented: $showFolders, arrowEdge: .bottom) {
                    FoldersPopover(model: shell.library)
                }
                Button {
                    Task { await shell.library.rescan() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .disabled(shell.library.isScanning)
                .help("Scan the machine again")
                .accessibilityLabel("Rescan")
            }
        }
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
                    FilterChip(label: candidate.label, isOn: facet == candidate) {
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
        let kinds: [SourceKind] = [.ollama, .huggingfaceCache, .lmStudio, .file, .folder]
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
                        GridItem(.adaptive(minimum: 260), spacing: Design.Space.l, alignment: .top)
                    ],
                    spacing: Design.Space.xxl
                ) {
                    ForEach(filtered) { record in
                        ModelCard(record: record) {
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
                    .foregroundStyle(Design.ink)
                Text(failure)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                    .padding(.top, Design.Space.s)
            } else if let summary = shell.library.summary {
                Text(summary.headline)
                    .font(Design.paneTitle)
                    .multilineTextAlignment(.center)
                    .lineSpacing(5)
                    .frame(maxWidth: 430)
            } else {
                Text("Looking for models on this Mac…")
                    .font(Design.paneTitle)
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
        case .file, .folder: "Loose"
        default: "Other"
        }
    }
}

struct ModelCard: View {
    let record: ModelRecord
    let onOpen: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: Design.Space.l) {
                HStack(alignment: .top, spacing: Design.Space.l) {
                    glyphTile
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        Text(record.name)
                            .font(Design.title)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                        Text(
                            "\(ModelsPane.storeTitle(record.source.kind)) · \(record.modality.rawValue)"
                        )
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    }
                    Spacer(minLength: 0)
                    if record.state == .missing {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(Design.glyphSmall)
                            .foregroundStyle(Design.inkSoft)
                            .help("No longer found on disk")
                    }
                }
                Spacer(minLength: 0)
                HStack(alignment: .firstTextBaseline) {
                    Text(
                        record.footprintMB.map {
                            DiscoverySummary.formatBytes(Int64($0) << 20)
                        } ?? "size unknown"
                    )
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkSoft)
                    Spacer()
                    Text(MetaGrid.tierWord(record.runtime.tier).uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, minHeight: 112, alignment: .topLeading)
            .tile(hovering: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("Show details")
        .accessibilityLabel(record.displayName)
        .accessibilityIdentifier("model-card-\(record.id)")
    }

    private var glyphTile: some View {
        Image(systemName: Design.modalityGlyph(record.modality))
            .font(Design.glyphPrimary)
            .foregroundStyle(Design.inkSoft)
            .frame(width: 36, height: 36)
            .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.card))
    }
}

struct ModelDetailSheet: View {
    let record: ModelRecord
    let shell: ShellModel
    let onClose: () -> Void
    @State private var reason: String?
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
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    if record.runtime.tier == .recipeNeeded {
                        Text(reason ?? "")
                            .font(Design.body)
                            .foregroundStyle(Design.inkSoft)
                            .lineSpacing(Design.bodyLineSpacing)
                    }
                    specs
                    if needsConfirmation {
                        VStack(alignment: .leading, spacing: Design.Space.m) {
                            MicroHeader(title: "Runtime")
                            Picker("Runtime", selection: $chosenRuntime) {
                                Text(runtimeLabel(record.runtime.id ?? ""))
                                    .tag(record.runtime.id ?? "")
                                ForEach(record.runtime.alternatives, id: \.self) { alt in
                                    Text(runtimeLabel(alt)).tag(alt)
                                }
                            }
                            .pickerStyle(.radioGroup)
                            .labelsHidden()
                            Text("Nothing runs until you open it.")
                                .font(Design.label)
                                .foregroundStyle(Design.inkFaint)
                        }
                    }
                    if record.runtime.tier != .recipeNeeded {
                        Rectangle()
                            .fill(Design.hairline)
                            .frame(height: Design.hairlineWidth)
                            .padding(.vertical, Design.Space.xs)
                        ModelConfigureSection(record: record, shell: shell)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            }
            footer
                .padding(.horizontal, Design.Space.gutter)
                .padding(.bottom, Design.Space.gutter)
        }
        .frame(width: 440, height: record.runtime.tier == .recipeNeeded ? 420 : 560)
        .task(id: record.id) {
            guard record.runtime.tier == .recipeNeeded else { return }
            let record = record
            reason = await Task.detached {
                RecipeReason.text(for: record, format: Identification.identify(record).format)
            }.value
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            Image(systemName: Design.modalityGlyph(record.modality))
                .font(Design.glyphPrimary)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 40, height: 40)
                .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.inner))
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(record.displayName)
                    .font(Design.title)
                    .lineLimit(1)
                Text(headerSubtitle)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            Button {
                onClose()
            } label: {
                Image(systemName: "xmark")
                    .font(Design.glyphSmall.weight(.bold))
                    .foregroundStyle(Design.inkSoft)
                    .frame(width: 24, height: 24)
                    .background(Design.cardFill, in: Circle())
            }
            .buttonStyle(.plain)
            .keyboardShortcut(.cancelAction)
            .accessibilityLabel("Close")
        }
    }

    private var headerSubtitle: String {
        if let runtime = record.runtime.id {
            return "runs via \(runtime) · \(MetaGrid.tierWord(record.runtime.tier))"
        }
        return MetaGrid.tierWord(record.runtime.tier)
    }

    private var specs: some View {
        VStack(alignment: .leading, spacing: 0) {
            if record.displayName != record.name {
                specRow("Source", record.name)
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
            if let path = record.primaryWeightPath ?? record.source.path as String? {
                Divider()
                specRow("Path", (path as NSString).abbreviatingWithTildeInPath)
            }
        }
    }

    private func specRow(_ label: String, _ value: String, mono: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .frame(width: 72, alignment: .leading)
            Text(value)
                .font(mono ? Design.data(12) : Design.caption)
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
            Text("A runtime recipe can make this model runnable later.")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
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
        id == record.runtime.id ? "\(id) — suggested" : id
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
    nonisolated static func text(for record: ModelRecord, format: ModelFormat) -> String {
        switch format {
        case .unknown:
            return record.primaryWeightPath == nil
                ? "This model's weights are in a format none of the built-in runtimes can execute — no safetensors or GGUF detected, likely PyTorch or another framework's format."
                : "Hedos found this model but could not identify what kind it is."
        case .diffusers:
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

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            MicroHeader(title: "Configure")
                .padding(.bottom, Design.Space.m)
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
                    InkTextArea(placeholder: "Optional", text: $promptDraft)
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
            Text("Auto means the model decides — nothing is sent unless you set it.")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .padding(.top, Design.Space.m)
        }
        .onAppear { seedDrafts() }
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
                ParamControl(
                    spec: spec,
                    get: { record.paramValues[spec.key] },
                    set: { value in write(spec.key, value) })
            }
            .padding(.vertical, Design.Space.chipX)
        } else {
            HStack(alignment: .center, spacing: Design.Space.s) {
                parameterLabel(spec)
                Spacer(minLength: Design.Space.l)
                ParamControl(
                    spec: spec,
                    get: { record.paramValues[spec.key] },
                    set: { value in write(spec.key, value) })
                    .frame(width: 190)
            }
            .padding(.vertical, Design.Space.chipX)
        }
    }

    private func parameterLabel(_ spec: ParamSpec) -> some View {
        HStack(spacing: Design.Space.s) {
            Text(humanized(spec.key))
                .font(Design.caption)
                .foregroundStyle(
                    record.paramValues[spec.key] != nil ? Design.ink : Design.inkSoft)
            if record.paramValues[spec.key] != nil {
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

    private func write(_ key: String, _ value: JSONValue?) {
        let shell = shell
        let id = record.id
        Task {
            try? await shell.kernel.setParamValue(id, key: key, to: value)
            await shell.library.refreshShelf()
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
