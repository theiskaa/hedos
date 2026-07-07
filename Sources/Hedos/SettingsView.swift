import AppKit
import HedosKernel
import SwiftUI

struct SettingsEntry: Identifiable {
    let id: String
    let section: String
    let title: String
    let keywords: [String]

    func matches(_ query: String) -> Bool {
        let needle = query.lowercased()
        return title.lowercased().contains(needle)
            || section.lowercased().contains(needle)
            || keywords.contains { $0.lowercased().contains(needle) }
    }
}

enum SettingsIndex {
    static let entries: [SettingsEntry] = [
        .init(
            id: "general.restore", section: "General", title: "Restore last session",
            keywords: ["launch", "startup", "open", "resume"]),
        .init(
            id: "general.startMode", section: "General", title: "Start in",
            keywords: ["launch", "startup", "mode", "default screen"]),
        .init(
            id: "general.defaultModel", section: "General", title: "Default chat model",
            keywords: ["model", "chat", "new chat"]),
        .init(
            id: "models.keepWarm", section: "Models", title: "Keep models warm",
            keywords: ["warm", "residency", "memory", "unload", "idle"]),
        .init(
            id: "models.eviction", section: "Models", title: "Eviction",
            keywords: ["memory", "ram", "unload", "single", "budget"]),
        .init(
            id: "models.budget", section: "Models", title: "RAM budget",
            keywords: ["ram", "memory", "budget", "limit"]),
        .init(
            id: "models.folders", section: "Models", title: "Watched folders",
            keywords: ["folder", "scan", "discovery", "watch"]),
        .init(
            id: "chat.prompt", section: "Chat", title: "Default system prompt",
            keywords: ["system", "prompt", "instructions"]),
        .init(
            id: "chat.send", section: "Chat", title: "Send with Return",
            keywords: ["enter", "return", "send", "newline", "keyboard"]),
        .init(
            id: "chat.stats", section: "Chat", title: "Show generation stats",
            keywords: ["stats", "tokens", "speed", "ttft"]),
        .init(
            id: "chat.export", section: "Chat", title: "Default export format",
            keywords: ["export", "markdown", "json"]),
        .init(
            id: "voice.default", section: "Voice", title: "Default voice",
            keywords: ["voice", "speaker", "tts"]),
        .init(
            id: "voice.speed", section: "Voice", title: "Speed",
            keywords: ["voice", "rate", "speed", "tts"]),
        .init(
            id: "appearance.theme", section: "Appearance", title: "Theme",
            keywords: ["dark", "light", "system", "appearance"]),
        .init(
            id: "appearance.width", section: "Appearance", title: "Chat width",
            keywords: ["wide", "comfortable", "layout"]),
        .init(
            id: "appearance.density", section: "Appearance", title: "Density",
            keywords: ["compact", "relaxed", "spacing"]),
        .init(
            id: "advanced.history", section: "Advanced", title: "Job history length",
            keywords: ["jobs", "history", "limit"]),
        .init(
            id: "advanced.paths", section: "Advanced", title: "Data locations",
            keywords: ["path", "registry", "database", "reveal", "folder", "support"]),
    ]
}

@Observable
@MainActor
final class SettingsModel {
    private let kernel: Kernel
    private var saveTasks: [String: Task<Void, Never>] = [:]
    private let previewPlayer = PCMPlayer()
    private var previewTask: Task<Void, Never>?

    var general = GeneralSettings()
    var models = ModelsSettings()
    var chat = ChatSettings()
    var voice = VoiceSettings()
    var appearance = AppearanceSettings()
    var advanced = AdvancedSettings()
    var voices: [String] = []
    var previewing = false
    private(set) var loaded = false

    static weak var active: SettingsModel?

    init(kernel: Kernel) {
        self.kernel = kernel
        Self.active = self
    }

    func load() async {
        general = await kernel.generalSettings()
        models = await kernel.modelsSettings()
        chat = await kernel.chatSettings()
        voice = await kernel.voiceSettings()
        appearance = await kernel.appearanceSettings()
        advanced = await kernel.advancedSettings()
        loaded = true
        applyTheme()
    }

    func applyTheme() {
        NSApp.appearance = appearance.theme.nsAppearance
    }

    func loadVoices(from records: [ModelRecord]) async {
        guard
            let speaker = records.first(where: {
                $0.state == .ready && Launcher.destination(for: $0) == .voice
            })
        else {
            voices = []
            return
        }
        voices = (try? await kernel.voices(speaker.id)) ?? []
    }

    func saveGeneral() {
        let value = general
        persist("general") { kernel in
            try? await kernel.updateGeneralSettings(value)
        }
    }

    func saveModels() {
        let value = models
        persist("models") { kernel in
            try? await kernel.updateModelsSettings(value)
        }
    }

    func saveChat() {
        let value = chat
        persist("chat") { kernel in
            try? await kernel.updateChatSettings(value)
        }
    }

    func saveVoice() {
        let value = voice
        persist("voice") { kernel in
            try? await kernel.updateVoiceSettings(value)
        }
    }

    func saveAppearance() {
        applyTheme()
        let value = appearance
        persist("appearance") { kernel in
            try? await kernel.updateAppearanceSettings(value)
        }
    }

    func saveAdvanced() {
        let value = advanced
        persist("advanced") { kernel in
            try? await kernel.updateAdvancedSettings(value)
        }
    }

    func previewVoice(records: [ModelRecord]) {
        guard !previewing,
            let speaker = records.first(where: {
                $0.state == .ready && Launcher.destination(for: $0) == .voice
            })
        else { return }
        previewing = true
        let kernel = kernel
        let chosen = voice.defaultVoice ?? voices.first ?? ""
        previewTask = Task { [weak self] in
            defer { self?.previewing = false }
            guard let self else { return }
            do {
                let stream = try await kernel.invoke(
                    speaker.id, .speak,
                    payload: .object([
                        "text": .string("Hedos speaks with this voice."),
                        "voice": .string(chosen),
                    ]))
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        self.previewPlayer.enqueue(frame)
                    }
                }
            } catch {}
        }
    }

    var directory: URL {
        kernel.directory
    }

    nonisolated var kernelRef: Kernel {
        kernel
    }

    func flush() async {
        for task in saveTasks.values {
            task.cancel()
        }
        saveTasks = [:]
        try? await kernel.updateGeneralSettings(general)
        try? await kernel.updateModelsSettings(models)
        try? await kernel.updateChatSettings(chat)
        try? await kernel.updateVoiceSettings(voice)
        try? await kernel.updateAppearanceSettings(appearance)
        try? await kernel.updateAdvancedSettings(advanced)
    }

    private func persist(_ key: String, _ operation: @escaping (Kernel) async -> Void) {
        saveTasks[key]?.cancel()
        let kernel = kernel
        saveTasks[key] = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            await operation(kernel)
        }
    }
}

private struct SettingRow<Control: View>: View {
    let id: String
    let label: String
    let highlighted: Bool
    @ViewBuilder let control: () -> Control
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center) {
            Text(label)
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
            Spacer(minLength: Design.Space.l)
            control()
        }
        .padding(.vertical, Design.Space.chipX)
        .padding(.horizontal, Design.Space.s)
        .background(
            RoundedRectangle(cornerRadius: Design.Radius.card)
                .fill(
                    highlighted
                        ? Design.ink.opacity(0.08)
                        : hovering ? Design.ink.opacity(0.02) : .clear))
        .padding(.horizontal, -Design.Space.s)
        .onHover { hovering = $0 }
        .id(id)
    }
}

enum SettingsSection: String, CaseIterable {
    case general
    case models
    case chat
    case voice
    case appearance
    case advanced

    var title: String {
        switch self {
        case .general: "General"
        case .models: "Models"
        case .chat: "Chat"
        case .voice: "Voice"
        case .appearance: "Appearance"
        case .advanced: "Advanced"
        }
    }

    var glyph: String {
        switch self {
        case .general: "gearshape"
        case .models: "square.stack.3d.up"
        case .chat: "message"
        case .voice: "speaker.wave.2"
        case .appearance: "paintpalette"
        case .advanced: "slider.horizontal.3"
        }
    }

    var blurb: String {
        switch self {
        case .general: "App behavior and launch."
        case .models: "Residency, memory, and discovery."
        case .chat: "Prompts, sending, and exports."
        case .voice: "Voices, speed, and speaking."
        case .appearance: "Theme and layout. Previews always show both palettes."
        case .advanced: "History and data locations."
        }
    }
}

struct SettingsDestination: Equatable {
    let section: SettingsSection
    var anchor: String?
}

extension AppearanceSettings.Theme {
    var nsAppearance: NSAppearance? {
        switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
    }
}

struct SettingsRoot: View {
    @Bindable var shell: ShellModel
    @State private var query = ""
    @State private var highlighted: String?
    @State private var selected: SettingsSection = .general
    @State private var hoveredSection: SettingsSection?
    @State private var warmCount = 0
    @State private var collapsed = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var model: SettingsModel { shell.settings }

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Rectangle()
                .fill(Design.line)
                .frame(width: 1)
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .ignoresSafeArea(.container, edges: .top)
        .scrollEdgeEffectStyle(.none, for: .top)
        .background(Design.paper.ignoresSafeArea())
        .task {
            if !shell.settings.loaded {
                await shell.settings.load()
            }
            await shell.settings.loadVoices(from: shell.library.records)
        }
        .task {
            while !Task.isCancelled {
                let fresh = await shell.kernel.residentModels().count
                if fresh != warmCount {
                    warmCount = fresh
                }
                try? await Task.sleep(for: .seconds(10))
            }
        }
        .onChange(of: shell.settingsTarget) { _, target in
            guard let target else { return }
            navigate(to: target)
            shell.settingsTarget = nil
        }
    }

    private var sidebar: some View {
        CollapsingSidebar(collapsed: collapsed) {
            expandedSidebar
        } collapsedContent: {
            collapsedSidebar
        }
        .accessibilityIdentifier("settings-sidebar")
    }

    private var expandedSidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Design.Space.s) {
                InkSearchField(placeholder: "Search settings", query: $query)
                collapser
            }
            .padding(.bottom, Design.Space.l)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    expandedGroup("App", [.general, .appearance])
                    expandedGroup("Surfaces", [.chat, .voice])
                    expandedGroup("Library", [.models])
                    expandedGroup("System", [.advanced])
                }
                .padding(.bottom, Design.Space.l)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
        }
        .padding(.top, Design.Space.pane + Design.Space.l)
        .padding(.horizontal, Design.Space.l)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private var collapsedSidebar: some View {
        VStack(alignment: .center, spacing: 0) {
            collapser
                .padding(.bottom, Design.Space.l)
            ScrollView {
                VStack(alignment: .center, spacing: Design.Space.xs) {
                    collapsedGroup([.general, .appearance], first: true)
                    collapsedGroup([.chat, .voice])
                    collapsedGroup([.models])
                    collapsedGroup([.advanced])
                }
                .padding(.bottom, Design.Space.l)
                .frame(maxWidth: .infinity)
            }
            .scrollIndicators(.hidden)
        }
        .padding(.top, Design.Space.pane + Design.Space.l)
        .padding(.horizontal, Design.Space.m)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private var collapser: some View {
        SidebarCollapseToggle(collapsed: collapsed) {
            withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                collapsed.toggle()
                if collapsed {
                    query = ""
                }
            }
        }
        .accessibilityIdentifier("settings-collapse")
    }

    @ViewBuilder
    private func expandedGroup(
        _ title: String, _ sections: [SettingsSection]
    ) -> some View {
        Text(title.uppercased())
            .font(Design.micro)
            .tracking(Design.microTracking)
            .foregroundStyle(Design.inkFaint)
            .padding(.horizontal, Design.Space.l)
            .padding(.top, Design.Space.l)
            .padding(.bottom, Design.Space.xxs)
        ForEach(sections, id: \.self) { section in
            sectionRow(section, collapsedRow: false)
        }
    }

    @ViewBuilder
    private func collapsedGroup(
        _ sections: [SettingsSection], first: Bool = false
    ) -> some View {
        if !first {
            Rectangle()
                .fill(Design.line)
                .frame(width: 28, height: Design.hairlineWidth)
                .padding(.vertical, Design.Space.s)
                .accessibilityHidden(true)
        }
        ForEach(sections, id: \.self) { section in
            sectionRow(section, collapsedRow: true)
        }
    }

    private func sectionRow(_ section: SettingsSection, collapsedRow: Bool) -> some View {
        InkSidebarRow(
            id: section,
            glyph: section.glyph,
            title: section.title,
            annotation: section == .models && warmCount > 0
                ? "\(warmCount) warm" : nil,
            selected: selected == section && query.isEmpty,
            collapsed: collapsedRow,
            hovered: $hoveredSection
        ) {
            query = ""
            withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                selected = section
            }
        }
        .accessibilityIdentifier("settings-\(section.rawValue)")
    }

    private func navigate(to destination: SettingsDestination) {
        query = ""
        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
            selected = destination.section
        }
        if let anchor = destination.anchor {
            pendingScroll = anchor
        }
    }

    @State private var pendingScroll: String?

    private var detail: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xxl) {
                    if !query.isEmpty {
                        searchResults(proxy: proxy)
                    } else {
                        VStack(alignment: .leading, spacing: Design.Space.xxs) {
                            Text(selected.title)
                                .font(Design.title)
                                .tracking(Design.tightTracking)
                                .foregroundStyle(Design.ink)
                            Text(selected.blurb)
                                .font(Design.caption)
                                .foregroundStyle(Design.inkSoft)
                        }
                        switch selected {
                        case .general: generalSection
                        case .models: modelsSection
                        case .chat: chatSection
                        case .voice: voiceSection
                        case .appearance: appearanceSection
                        case .advanced: advancedSection
                        }
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.pane)
                .padding(.bottom, Design.Space.xxl)
                .frame(maxWidth: 640, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onChange(of: pendingScroll) { _, anchor in
                guard let anchor else { return }
                Task {
                    try? await Task.sleep(for: .milliseconds(150))
                    withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                        proxy.scrollTo(anchor, anchor: .center)
                    }
                    flash(anchor)
                    pendingScroll = nil
                }
            }
        }
    }

    private func searchResults(proxy: ScrollViewProxy) -> some View {
        let matches = SettingsIndex.entries.filter { $0.matches(query) }
        return VStack(alignment: .leading, spacing: Design.Space.xxs) {
            MicroHeader(title: "Results")
                .padding(.bottom, Design.Space.xxs)
            if matches.isEmpty {
                Text("Nothing matches.")
                    .font(Design.caption)
                    .foregroundStyle(Design.inkFaint)
            }
            ForEach(matches) { entry in
                InkMenuRow(
                    title: entry.title,
                    annotation: entry.section
                ) {
                    if let target = SettingsSection.allCases.first(where: {
                        $0.title == entry.section
                    }) {
                        navigate(to: SettingsDestination(section: target, anchor: entry.id))
                    }
                }
            }
        }
        .padding(Design.Space.tile)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private func flash(_ id: String) {
        highlighted = id
        Task {
            try? await Task.sleep(for: .milliseconds(1200))
            if highlighted == id {
                highlighted = nil
            }
        }
    }

    private var generalSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Launch") {
            settingRow("general.restore", "Restore last session") {
                InkToggle(
                    isOn: model.general.restoreLastSession, isSet: true,
                    onToggle: { value in
                        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                            model.general.restoreLastSession = value
                        }
                        model.saveGeneral()
                    },
                    label: "Restore last session")
            }
            Divider()
            settingRow("general.startMode", "Start in") {
                InkDropdown(
                    options: AppMode.allCases.filter { $0 != .settings }
                        .map { Design.modeTitle($0) },
                    selection: model.general.fixedMode.map { Design.modeTitle($0) },
                    placeholder: "Models",
                    accessibilityName: "start mode",
                    onSelect: { title in
                        model.general.fixedMode = AppMode.allCases.first {
                            Design.modeTitle($0) == title
                        }
                        model.saveGeneral()
                    })
            }
            .disabled(model.general.restoreLastSession)
            .opacity(model.general.restoreLastSession ? 0.4 : 1)
            .animation(
                Design.motion(reduceMotion: reduceMotion),
                value: model.general.restoreLastSession)
            }
            group("Defaults") {
            settingRow("general.defaultModel", "Default chat model") {
                InkDropdown(
                    options: readyChatModels.map(\.displayName),
                    selection: defaultChatModelName,
                    placeholder: "None",
                    accessibilityName: "default chat model",
                    onSelect: { name in
                        let record = readyChatModels.first { $0.displayName == name }
                        model.chat.defaultModelID = record?.id
                        model.saveChat()
                    })
            }
        }
        }
    }


    private var modelsSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Residency") {
                settingRow("models.keepWarm", "Keep models warm") {
                    InkSegmented(
                        values: ["5 min", "15 min", "1 h", "Never"],
                        selection: keepWarmLabel(model.models.keepWarm),
                        onSelect: { label in
                            model.models.keepWarm = keepWarmValue(label)
                            model.saveModels()
                        })
                }
                residencyStrip
                Divider()
                settingRow("models.eviction", "Eviction") {
                    InkSegmented(
                        values: ["Single", "Budgeted"],
                        selection: model.models.eviction == .strictSingle
                            ? "Single" : "Budgeted",
                        onSelect: { label in
                            model.models.eviction =
                                label == "Single" ? .strictSingle : .budgeted
                            model.saveModels()
                        })
                }
                Divider()
                settingRow("models.budget", "RAM budget") {
                    HStack(spacing: Design.Space.m) {
                        InkSlider(
                            range: 4...64,
                            value: Double((model.models.ramBudgetMB ?? 16384) / 1024),
                            isSet: model.models.ramBudgetMB != nil,
                            onChange: { value in
                                model.models.ramBudgetMB = Int(value.rounded()) * 1024
                                model.saveModels()
                            },
                            label: "RAM budget")
                        Text(
                            model.models.ramBudgetMB.map { "\($0 / 1024) GB" } ?? "auto"
                        )
                        .font(Design.data(11))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                        .foregroundStyle(
                            model.models.ramBudgetMB == nil ? Design.inkFaint : Design.ink)
                        .frame(minWidth: 44, alignment: .trailing)
                    }
                    .frame(width: 220)
                }
                .disabled(model.models.eviction != .budgeted)
                .opacity(model.models.eviction == .budgeted ? 1 : 0.4)
                .animation(
                    Design.motion(reduceMotion: reduceMotion),
                    value: model.models.eviction)
                if model.models.eviction == .budgeted {
                    budgetBar
                        .transition(.opacity)
                }
            }
            group("Watched folders") {
                foldersRows
            }
        }
    }

    private var residencyStrip: some View {
        ResidencyStrip(shell: shell)
    }

    private var budgetBar: some View {
        BudgetBar(shell: shell)
    }

    private var foldersRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                Spacer()
                Button("Add…") {
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.canChooseFiles = false
                    panel.allowsMultipleSelection = false
                    if panel.runModal() == .OK, let url = panel.url {
                        let shell = shell
                        Task {
                            await shell.library.addFolder(url)
                        }
                    }
                }
                .buttonStyle(QuietButtonStyle())
            }
            ForEach(shell.library.watchedFolders, id: \.self) { path in
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: .folder, size: 12)
                        .foregroundStyle(Design.inkFaint)
                    Text((path as NSString).abbreviatingWithTildeInPath)
                        .font(Design.label)
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Button {
                        let shell = shell
                        Task {
                            await shell.library.removeFolder(path)
                        }
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(Design.glyphInline)
                            .foregroundStyle(Design.inkFaint)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Stop watching \(path)")
                }
            }
            if shell.library.watchedFolders.isEmpty {
                Text("Standard locations are always scanned.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.vertical, Design.Space.m)
        .id("models.folders")
        .background(highlightBackground("models.folders"))
    }

    private var chatSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("System prompt") {
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    InkTextArea(
                        placeholder: "Used when a model has no prompt of its own",
                        text: Binding(
                            get: { model.chat.defaultSystemPrompt ?? "" },
                            set: { value in
                                model.chat.defaultSystemPrompt = value.isEmpty ? nil : value
                                model.saveChat()
                            }),
                        resizable: true)
                }
                .padding(.vertical, Design.Space.m)
                .id("chat.prompt")
                .background(highlightBackground("chat.prompt"))
            }
            group("Behavior") {
                settingRow("chat.send", "Send with Return") {
                    InkToggle(
                        isOn: model.chat.sendWithEnter, isSet: true,
                        onToggle: { value in
                            model.chat.sendWithEnter = value
                            model.saveChat()
                        },
                        label: "Send with Return")
                }
                Divider()
                settingRow("chat.stats", "Show generation stats") {
                    InkToggle(
                        isOn: model.chat.showStats, isSet: true,
                        onToggle: { value in
                            withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                                model.chat.showStats = value
                            }
                            model.saveChat()
                        },
                        label: "Show generation stats")
                }
                if model.chat.showStats {
                    Text("1.2s to first token · 34 tok/s")
                        .font(Design.data(10))
                        .foregroundStyle(Design.inkFaint)
                        .padding(.bottom, Design.Space.m)
                        .transition(.opacity)
                }
                Divider()
                settingRow("chat.export", "Default export format") {
                    InkSegmented(
                        values: ["Markdown", "JSON"],
                        selection: model.chat.exportFormat == .markdown
                            ? "Markdown" : "JSON",
                        onSelect: { label in
                            model.chat.exportFormat =
                                label == "Markdown" ? .markdown : .json
                            model.saveChat()
                        })
                }
            }
        }
    }

    private var voiceSection: some View {
        @Bindable var model = shell.settings
        return group("Speaking") {
            settingRow("voice.default", "Default voice") {
                HStack(spacing: Design.Space.m) {
                    InkDropdown(
                        options: model.voices,
                        selection: model.voice.defaultVoice,
                        accessibilityName: "default voice",
                        onSelect: { choice in
                            model.voice.defaultVoice = choice
                            model.saveVoice()
                        })
                    if model.previewing {
                        SpeakingIndicator()
                    }
                    Button(model.previewing ? "Playing…" : "Preview") {
                        model.previewVoice(records: shell.library.records)
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(model.voices.isEmpty || model.previewing)
                }
            }
            Divider()
            settingRow("voice.speed", "Speed") {
                HStack(spacing: Design.Space.m) {
                    InkSlider(
                        range: 0.5...2.0,
                        value: model.voice.speed,
                        isSet: model.voice.speed != 1.0,
                        onChange: { value in
                            model.voice.speed = (value * 10).rounded() / 10
                            model.saveVoice()
                        },
                        label: "Speed")
                    Text(String(format: "%.1f×", model.voice.speed))
                        .font(Design.data(11))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                        .animation(
                            Design.motion(reduceMotion: reduceMotion),
                            value: model.voice.speed)
                        .foregroundStyle(Design.ink)
                        .frame(minWidth: 34, alignment: .trailing)
                }
                .frame(width: 220)
            }
        }
    }

    private var appearanceSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Theme") {
                cardChoiceRow("appearance.theme", "Palette") {
                    InkChoiceCard(
                        label: "System",
                        selected: model.appearance.theme == .system,
                        action: {
                            model.appearance.theme = .system
                            model.saveAppearance()
                        }
                    ) {
                        ThemePreview(variant: .system)
                    }
                    InkChoiceCard(
                        label: "Light",
                        selected: model.appearance.theme == .light,
                        action: {
                            model.appearance.theme = .light
                            model.saveAppearance()
                        }
                    ) {
                        ThemePreview(variant: .light)
                    }
                    InkChoiceCard(
                        label: "Dark",
                        selected: model.appearance.theme == .dark,
                        action: {
                            model.appearance.theme = .dark
                            model.saveAppearance()
                        }
                    ) {
                        ThemePreview(variant: .dark)
                    }
                }
            }
            group("Layout") {
                HStack(alignment: .top, spacing: Design.Space.gutter) {
                    cardChoiceRow("appearance.width", "Chat width") {
                        InkChoiceCard(
                            label: "Comfortable",
                            selected: model.appearance.chatWidth == .comfortable,
                            action: {
                                model.appearance.chatWidth = .comfortable
                                model.saveAppearance()
                            }
                        ) {
                            WidthPreview(wide: false)
                        }
                        InkChoiceCard(
                            label: "Wide",
                            selected: model.appearance.chatWidth == .wide,
                            action: {
                                model.appearance.chatWidth = .wide
                                model.saveAppearance()
                            }
                        ) {
                            WidthPreview(wide: true)
                        }
                    }
                    cardChoiceRow("appearance.density", "Density") {
                        InkChoiceCard(
                            label: "Relaxed",
                            selected: model.appearance.density == .relaxed,
                            action: {
                                model.appearance.density = .relaxed
                                model.saveAppearance()
                            }
                        ) {
                            DensityPreview(compact: false)
                        }
                        InkChoiceCard(
                            label: "Compact",
                            selected: model.appearance.density == .compact,
                            action: {
                                model.appearance.density = .compact
                                model.saveAppearance()
                            }
                        ) {
                            DensityPreview(compact: true)
                        }
                    }
                }
            }
        }
    }

    private func cardChoiceRow(
        _ id: String, _ label: String, @ViewBuilder cards: () -> some View
    ) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Text(label.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
            HStack(spacing: Design.Space.m) {
                cards()
            }
        }
        .padding(.vertical, Design.Space.chipX)
        .id(id)
        .background(highlightBackground(id))
    }

    private var advancedSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Jobs") {
                settingRow("advanced.history", "Job history length") {
                    HStack(spacing: Design.Space.m) {
                        InkSlider(
                            range: 10...500,
                            value: Double(model.advanced.jobHistoryLimit),
                            isSet: true,
                            onChange: { value in
                                model.advanced.jobHistoryLimit = Int(value.rounded())
                                model.saveAdvanced()
                            },
                            label: "Job history length")
                        Text("\(model.advanced.jobHistoryLimit)")
                            .font(Design.data(11))
                            .monospacedDigit()
                            .contentTransition(.numericText())
                            .animation(
                                Design.motion(reduceMotion: reduceMotion),
                                value: model.advanced.jobHistoryLimit)
                            .foregroundStyle(Design.ink)
                            .frame(minWidth: 34, alignment: .trailing)
                    }
                    .frame(width: 220)
                }
            }
            group("Data locations") {
                VStack(alignment: .leading, spacing: Design.Space.s) {
                    pathRow("Support folder", model.directory.path)
                    pathRow(
                        "Chats database",
                        model.directory.appendingPathComponent("chats.sqlite").path)
                    pathRow(
                        "Settings",
                        model.directory.appendingPathComponent(
                            "settings", isDirectory: true
                        ).path)
                }
                .padding(.vertical, Design.Space.m)
                .id("advanced.paths")
                .background(highlightBackground("advanced.paths"))
            }
        }
    }

    private func pathRow(_ label: String, _ path: String) -> some View {
        HStack(spacing: Design.Space.s) {
            Text(label)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .frame(width: 110, alignment: .leading)
            Text((path as NSString).abbreviatingWithTildeInPath)
                .font(Design.data(10))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer()
            Button("Reveal") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
            .buttonStyle(QuietButtonStyle())
        }
    }

    private func group(
        _ header: String, @ViewBuilder content: () -> some View
    ) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: header)
            VStack(alignment: .leading, spacing: 0) {
                content()
            }
            .padding(.horizontal, Design.Space.tile)
            .padding(.vertical, Design.Space.xs)
            .surfaceCard(radius: Design.Radius.tile)
            .shadow(color: Design.shadowColor.opacity(0.06), radius: 14, x: 0, y: 5)
        }
    }

    private func settingRow<Control: View>(
        _ id: String, _ label: String, @ViewBuilder control: @escaping () -> Control
    ) -> some View {
        SettingRow(id: id, label: label, highlighted: highlighted == id, control: control)
    }

    private func highlightBackground(_ id: String) -> some View {
        RoundedRectangle(cornerRadius: Design.Radius.card)
            .fill(highlighted == id ? Design.ink.opacity(0.08) : .clear)
            .padding(.horizontal, -Design.Space.s)
            .animation(
                reduceMotion ? nil : .easeOut(duration: 0.4), value: highlighted)
    }

    private var readyChatModels: [ModelRecord] {
        shell.library.records.filter {
            $0.state == .ready && Launcher.destination(for: $0) == .chat
        }
    }

    private var defaultChatModelName: String? {
        guard let id = shell.settings.chat.defaultModelID else { return nil }
        return shell.library.record(id: id)?.displayName
    }

    private func keepWarmLabel(_ policy: KeepWarmPolicy) -> String {
        switch policy {
        case .fiveMinutes: "5 min"
        case .fifteenMinutes: "15 min"
        case .oneHour: "1 h"
        case .never: "Never"
        }
    }

    private func keepWarmValue(_ label: String) -> KeepWarmPolicy {
        switch label {
        case "5 min": .fiveMinutes
        case "15 min": .fifteenMinutes
        case "1 h": .oneHour
        default: .never
        }
    }

}


struct ResidencyStrip: View {
    let shell: ShellModel
    @State private var resident: [Kernel.ResidentEntry] = []
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            HStack(spacing: Design.Space.s) {
                Circle()
                    .fill(resident.isEmpty ? Design.inkFaint : Design.ink)
                    .frame(width: 5, height: 5)
                if resident.isEmpty {
                    Text("Nothing resident right now.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                } else {
                    Text(line)
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
            }
            if resident.contains(where: { $0.origin == .ollama }) {
                Text("Ollama models follow Ollama's own keep-alive, not these settings.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.bottom, Design.Space.m)
        .animation(Design.motion(reduceMotion: reduceMotion), value: resident)
        .task {
            while !Task.isCancelled {
                let fresh = await shell.kernel.residentModels()
                if fresh != resident {
                    resident = fresh
                }
                try? await Task.sleep(for: .seconds(8))
            }
        }
    }

    private var line: String {
        let parts = resident.map { entry in
            let name =
                entry.modelID.flatMap { shell.library.record(id: $0)?.displayName }
                ?? shell.library.records.first { $0.name == entry.name }?.displayName
                ?? entry.name
            let size = DiscoverySummary.formatBytes(Int64(entry.footprintMB) << 20)
            let origin = entry.origin == .ollama ? " · via Ollama" : ""
            return "\(name) · \(size)\(origin)"
        }
        return "Warm now: " + parts.joined(separator: "   ")
    }
}

private struct BudgetBar: View {
    let shell: ShellModel
    @State private var usedMB = 0
    @State private var fallbackBudgetMB = Int(ProcessInfo.processInfo.physicalMemory >> 20)

    var body: some View {
        let totalMB = Int(ProcessInfo.processInfo.physicalMemory >> 20)
        let budgetMB = shell.settings.models.ramBudgetMB ?? fallbackBudgetMB
        GeometryReader { proxy in
            let width = proxy.size.width
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Design.line)
                    .frame(height: 5)
                Capsule()
                    .fill(Design.inkSoft)
                    .frame(
                        width: min(max(0, width * CGFloat(usedMB) / CGFloat(totalMB)), width),
                        height: 5)
                Rectangle()
                    .fill(Design.ink)
                    .frame(width: 2, height: 11)
                    .offset(
                        x: min(width * CGFloat(budgetMB) / CGFloat(totalMB), width) - 1)
            }
        }
        .frame(height: 11)
        .padding(.bottom, Design.Space.m)
        .accessibilityLabel(
            "Resident \(usedMB >> 10) of \(totalMB >> 10) gigabytes, budget \(budgetMB >> 10)")
        .task {
            fallbackBudgetMB = await shell.kernel.governor.defaultBudgetMB
            while !Task.isCancelled {
                let fresh = await shell.kernel.residentModels()
                    .reduce(0) { $0 + $1.footprintMB }
                if fresh != usedMB {
                    usedMB = fresh
                }
                try? await Task.sleep(for: .seconds(8))
            }
        }
    }
}
