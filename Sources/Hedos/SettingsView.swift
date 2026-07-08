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
            id: "general.quickAsk", section: "General", title: "Quick Ask shortcut",
            keywords: ["hotkey", "shortcut", "global", "keyboard", "panel", "spotlight"]),
        .init(
            id: "general.menuBar", section: "General", title: "Menu bar item",
            keywords: ["menu", "status", "bar", "tray", "icon"]),
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
            id: "models.hfCache", section: "Models", title: "Hugging Face caches",
            keywords: ["hf", "hugging", "face", "cache", "hub", "home", "huggingface"]),
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
            id: "voice.autoSpeak", section: "Voice", title: "Speak replies and narrations aloud",
            keywords: ["auto", "speak", "read", "aloud", "voice", "reply"]),
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
            id: "appearance.fontUI", section: "Appearance", title: "App font",
            keywords: ["font", "typeface", "typography", "family", "text"]),
        .init(
            id: "appearance.fontMono", section: "Appearance", title: "Mono font",
            keywords: ["font", "monospace", "mono", "code", "typography"]),
        .init(
            id: "prompts.library", section: "Prompts", title: "Prompt library",
            keywords: ["prompt", "slash", "template", "snippet", "insert", "placeholder"]),
        .init(
            id: "advanced.history", section: "Advanced", title: "Job history length",
            keywords: ["jobs", "history", "limit"]),
        .init(
            id: "advanced.paths", section: "Advanced", title: "Data locations",
            keywords: ["path", "registry", "database", "reveal", "folder", "support"]),
    ]
}

enum FontCatalog {
    static let uiFamilies: [String] = NSFontManager.shared.availableFontFamilies
        .filter { !$0.hasPrefix(".") }
        .sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }

    static let monoFamilies: [String] = uiFamilies.filter { family in
        guard
            let members = NSFontManager.shared.availableMembers(ofFontFamily: family),
            let first = members.first,
            let name = first.first as? String,
            let font = NSFont(name: name, size: 12)
        else { return false }
        return font.isFixedPitch
    }
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
    var prompts: [Prompt] = []
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
        prompts = await kernel.prompts()
        loaded = true
        applyTheme()
        applyShellIntegrations()
    }

    func updatePrompt(_ prompt: Prompt) {
        if let index = prompts.firstIndex(where: { $0.id == prompt.id }) {
            prompts[index] = prompt
        } else {
            prompts.append(prompt)
        }
        let value = prompt
        persist("prompt-\(prompt.id)") { kernel in
            guard !value.title.isEmpty || !value.body.isEmpty else { return }
            try? await kernel.savePrompt(value)
        }
    }

    func deletePrompt(_ prompt: Prompt) {
        saveTasks["prompt-\(prompt.id)"]?.cancel()
        prompts.removeAll { $0.id == prompt.id }
        let kernel = kernel
        let id = prompt.id
        Task {
            await kernel.deletePrompt(id: id)
        }
    }

    func applyTheme() {
        NSApp.appearance = appearance.theme.nsAppearance
        Design.fontBook = Design.FontBook(
            uiFamily: appearance.uiFont, monoFamily: appearance.monoFont)
    }

    func applyShellIntegrations() {
        HotkeyCenter.shared.apply(general.quickAskHotkey)
        MenuBarController.shared.apply(general.menuBarItem)
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
        applyShellIntegrations()
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
        let dirtyPromptIDs = saveTasks.keys
            .filter { $0.hasPrefix("prompt-") }
            .map { String($0.dropFirst("prompt-".count)) }
        for task in saveTasks.values {
            task.cancel()
        }
        saveTasks = [:]
        for id in dirtyPromptIDs {
            guard let prompt = prompts.first(where: { $0.id == id }),
                !prompt.title.isEmpty || !prompt.body.isEmpty
            else { continue }
            try? await kernel.savePrompt(prompt)
        }
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
    var caption: String? = nil
    let highlighted: Bool
    @ViewBuilder let control: () -> Control
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(label)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.ink)
                if let caption {
                    Text(caption)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .lineSpacing(1.5)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: Design.Space.l)
            control()
        }
        .padding(.vertical, Design.Space.l)
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
    case prompts
    case chat
    case voice
    case appearance
    case advanced

    var title: String {
        switch self {
        case .general: "General"
        case .models: "Models"
        case .prompts: "Prompts"
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
        case .prompts: "text.quote"
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
        case .prompts: "Reusable prompts, inserted from the composer with /."
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
        .id(Design.fontBook.identity)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .ignoresSafeArea(.container, edges: .top)
        .scrollEdgeEffectStyle(.none, for: .top)
        .background(Design.paper.ignoresSafeArea())
        .modalScrim(
            isPresented: promptDraft != nil,
            onDismiss: { promptDraft = nil }
        ) {
            if let draft = promptDraft {
                PromptSheet(
                    prompt: draft,
                    isNew: promptDraftIsNew,
                    onSave: { updated in
                        shell.settings.updatePrompt(updated)
                        promptDraft = nil
                    },
                    onDelete: promptDraftIsNew
                        ? nil
                        : {
                            shell.settings.deletePrompt(draft)
                            promptDraft = nil
                        },
                    onClose: { promptDraft = nil })
            }
        }
        .task {
            if !shell.settings.loaded {
                await shell.settings.load()
            }
            await shell.settings.loadVoices(from: shell.library.records)
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
                    expandedGroup("Library", [.models, .prompts])
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
                    collapsedGroup([.models, .prompts])
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
            annotation: section == .models && !shell.resident.isEmpty
                ? "\(shell.resident.count) warm" : nil,
            liveAnnotation: section == .models && !shell.resident.isEmpty,
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
    @State private var promptDraft: Prompt?
    @State private var promptDraftIsNew = false

    private var detail: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xxl) {
                    if !query.isEmpty {
                        searchResults(proxy: proxy)
                    } else {
                        HStack(alignment: .center, spacing: Design.Space.l) {
                            IconPlaque(size: 40) {
                                Image(systemName: selected.glyph)
                                    .font(Design.glyphNav)
                                    .foregroundStyle(Design.inkSoft)
                            }
                            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                                Text(selected.title)
                                    .font(Design.paneTitle)
                                    .tracking(Design.tightTracking)
                                    .foregroundStyle(Design.ink)
                                Text(selected.blurb)
                                    .font(Design.caption)
                                    .foregroundStyle(Design.inkSoft)
                            }
                        }
                        switch selected {
                        case .general: generalSection
                        case .models: modelsSection
                        case .chat: chatSection
                        case .voice: voiceSection
                        case .appearance: appearanceSection
                        case .prompts: promptsSection
                        case .advanced: advancedSection
                        }
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.pane)
                .padding(.bottom, Design.Space.xxl)
                .frame(maxWidth: Design.Column.settingsDetail, alignment: .leading)
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
            settingRow(
                "general.restore", "Restore last session",
                caption: "Reopen where you left off at launch.") {
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
            settingRow(
                "general.defaultModel", "Default chat model",
                caption: "New chats start bound to this model.") {
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
            group("Anywhere") {
                settingRow(
                    "general.quickAsk", "Quick Ask shortcut",
                    caption: "Summons a small ask panel from anywhere on this Mac.") {
                    HotkeyRecorder(hotkey: model.general.quickAskHotkey) { hotkey in
                        model.general.quickAskHotkey = hotkey
                        model.saveGeneral()
                    }
                }
                Divider()
                settingRow(
                    "general.menuBar", "Menu bar item",
                    caption: "A quiet bear in the menu bar; the dot means work is running.") {
                    InkToggle(
                        isOn: model.general.menuBarItem, isSet: true,
                        onToggle: { value in
                            model.general.menuBarItem = value
                            model.saveGeneral()
                        },
                        label: "Menu bar item")
                }
            }
        }
    }


    private var modelsSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Memory") {
                warmRows
                if model.models.eviction == .budgeted {
                    budgetBar
                        .transition(.opacity)
                }
                Divider()
                settingRow(
                "models.keepWarm", "Keep models warm",
                caption: "How long a model stays in memory after its last reply.") {
                    InkSegmented(
                        values: ["5 min", "15 min", "1 h", "Never"],
                        selection: keepWarmLabel(model.models.keepWarm),
                        onSelect: { label in
                            model.models.keepWarm = keepWarmValue(label)
                            model.saveModels()
                        })
                }
                Divider()
                settingRow(
                "models.eviction", "Eviction",
                caption: "Single keeps one model warm; Budgeted packs what fits.") {
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
                settingRow(
                "models.budget", "RAM budget",
                caption: "The ceiling for warm models under Budgeted eviction.") {
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
                    .frame(width: Design.Column.control)
                }
                .disabled(model.models.eviction != .budgeted)
                .opacity(model.models.eviction == .budgeted ? 1 : 0.4)
                .animation(
                    Design.motion(reduceMotion: reduceMotion),
                    value: model.models.eviction)
            }
            group("Watched folders") {
                foldersRows
            }
            group("Hugging Face caches") {
                hfCacheRows
            }
        }
    }

    private var warmRows: some View {
        ResidencyStrip(shell: shell)
    }

    private var budgetBar: some View {
        BudgetBar(shell: shell)
    }

    private var hfCacheRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                Spacer()
                Button("Add…") {
                    let shell = shell
                    FolderRow.pickFolder { url in
                        Task {
                            await shell.library.addHFRoot(url)
                        }
                    }
                }
                .buttonStyle(QuietButtonStyle())
            }
            ForEach(shell.library.hfCacheRoots, id: \.self) { path in
                FolderRow(path: path) {
                    let shell = shell
                    Task {
                        await shell.library.removeHFRoot(path)
                    }
                }
            }
            if shell.library.hfCacheRoots.isEmpty {
                Text("Standard locations and HF_HOME are always scanned.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.vertical, Design.Space.m)
        .id("models.hfCache")
        .background(highlightBackground("models.hfCache"))
    }

    private var foldersRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                Spacer()
                Button("Add…") {
                    let shell = shell
                    FolderRow.pickFolder { url in
                        Task {
                            await shell.library.addFolder(url)
                        }
                    }
                }
                .buttonStyle(QuietButtonStyle())
            }
            ForEach(shell.library.watchedFolders, id: \.self) { path in
                FolderRow(path: path) {
                    let shell = shell
                    Task {
                        await shell.library.removeFolder(path)
                    }
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
                settingRow(
                "chat.stats", "Show generation stats",
                caption: "Time to first token and speed under each reply.") {
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
                    .help(
                        model.voices.isEmpty
                            ? "Voices appear when a speech model is ready." : "")
                }
            }
            Divider()
            settingRow(
                "voice.autoSpeak", "Speak replies and narrations aloud",
                caption: "Replies and narrations play as soon as they finish.") {
                InkToggle(
                    isOn: model.voice.autoSpeak,
                    isSet: true,
                    onToggle: { value in
                        model.voice.autoSpeak = value
                        model.saveVoice()
                    },
                    label: "Speak replies and narrations aloud")
            }
            Divider()
            settingRow(
                "voice.speed", "Speed",
                caption: "Playback rate for every voice.") {
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
                .frame(width: Design.Column.control)
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
            group("Type") {
                settingRow("appearance.fontUI", "App font") {
                    InkDropdown(
                        options: FontCatalog.uiFamilies,
                        selection: model.appearance.uiFont,
                        placeholder: "San Francisco",
                        accessibilityName: "app font",
                        onSelect: { family in
                            model.appearance.uiFont = family
                            model.saveAppearance()
                        })
                }
                settingRow("appearance.fontMono", "Mono font") {
                    InkDropdown(
                        options: FontCatalog.monoFamilies,
                        selection: model.appearance.monoFont,
                        placeholder: "SF Mono",
                        accessibilityName: "mono font",
                        onSelect: { family in
                            model.appearance.monoFont = family
                            model.saveAppearance()
                        })
                }
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    Text("Sphinx of black quartz, judge my vow.")
                        .font(Design.body)
                        .foregroundStyle(Design.ink)
                    Text("SAMPLE 0123456789".uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
                .padding(.vertical, Design.Space.chipX)
                .padding(.horizontal, Design.Space.s)
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

    private var promptsSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            if shell.settings.prompts.isEmpty {
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    Text("No prompts yet.")
                        .font(Design.caption)
                        .foregroundStyle(Design.inkSoft)
                    Text(
                        "A prompt is a reusable message. Type / in any composer to insert one; {selection} pulls in whatever you had already drafted."
                    )
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineSpacing(2.5)
                    .frame(maxWidth: Design.Column.prose, alignment: .leading)
                }
                .padding(.vertical, Design.Space.chipX)
            }
            LazyVGrid(
                columns: [
                    GridItem(.adaptive(minimum: 250), spacing: Design.Space.l, alignment: .top)
                ],
                spacing: Design.Space.l
            ) {
                ForEach(shell.settings.prompts) { prompt in
                    PromptCard(prompt: prompt) {
                        promptDraft = prompt
                        promptDraftIsNew = false
                    } onDelete: {
                        shell.settings.deletePrompt(prompt)
                    }
                }
                NewPromptCard {
                    promptDraft = Prompt(title: "", body: "")
                    promptDraftIsNew = true
                }
            }
        }
        .id("prompts.library")
        .background(highlightBackground("prompts.library"))
    }

    private var advancedSection: some View {
        @Bindable var model = shell.settings
        return VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Jobs") {
                settingRow(
                "advanced.history", "Job history length",
                caption: "How many finished jobs stay in the log.") {
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
                    .frame(width: Design.Column.control)
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
        HStack(alignment: .center, spacing: Design.Space.l) {
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(label)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.ink)
                Text((path as NSString).abbreviatingWithTildeInPath)
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Spacer(minLength: Design.Space.l)
            Button("Reveal") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
            .buttonStyle(QuietButtonStyle())
        }
        .padding(.vertical, Design.Space.m)
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
        }
    }

    private func settingRow<Control: View>(
        _ id: String, _ label: String, caption: String? = nil,
        @ViewBuilder control: @escaping () -> Control
    ) -> some View {
        SettingRow(
            id: id, label: label, caption: caption, highlighted: highlighted == id,
            control: control)
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
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var resident: [Kernel.ResidentEntry] {
        shell.resident
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            if resident.isEmpty {
                HStack(spacing: Design.Space.s) {
                    Circle()
                        .fill(Design.inkFaint)
                        .frame(width: 5, height: 5)
                    Text("Nothing warm right now.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    Spacer()
                }
            } else {
                ForEach(resident, id: \.self) { entry in
                    HStack(spacing: Design.Space.chipX) {
                        AccentDot()
                        Text(name(entry))
                            .font(Design.caption.weight(.medium))
                            .foregroundStyle(Design.ink)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text(DiscoverySummary.formatBytes(Int64(entry.footprintMB) << 20))
                            .font(Design.data(11))
                            .foregroundStyle(Design.inkFaint)
                        if entry.origin == .ollama {
                            TintChip(text: "via Ollama")
                        }
                        Spacer(minLength: 0)
                    }
                }
            }
            if resident.contains(where: { $0.origin == .ollama }) {
                Text("Ollama models follow Ollama's own keep-alive, not these settings.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.vertical, Design.Space.m)
        .monospacedDigit()
        .animation(Design.motion(reduceMotion: reduceMotion), value: resident)
    }

    private func name(_ entry: Kernel.ResidentEntry) -> String {
        entry.modelID.flatMap { shell.library.record(id: $0)?.displayName }
            ?? shell.library.records.first { $0.name == entry.name }?.displayName
            ?? entry.name
    }
}

private struct BudgetBar: View {
    let shell: ShellModel

    var body: some View {
        let totalMB = Int(ProcessInfo.processInfo.physicalMemory >> 20)
        let fallbackMB =
            shell.residencyBudgetMB > 0
            ? shell.residencyBudgetMB : Int(ProcessInfo.processInfo.physicalMemory >> 20)
        let budgetMB = shell.settings.models.ramBudgetMB ?? fallbackMB
        let usedMB = shell.residentUsedMB
        GeometryReader { proxy in
            let width = proxy.size.width
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Design.line)
                    .frame(height: 8)
                Capsule()
                    .fill(Design.accent)
                    .frame(
                        width: min(max(0, width * CGFloat(usedMB) / CGFloat(totalMB)), width),
                        height: 8)
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
    }
}

private func promptCapabilityLabel(_ capability: Capability?) -> String? {
    switch capability {
    case .some(.chat): "Chat"
    case .some(.speak): "Speak"
    case .some(.image): "Image"
    case .some(let other): other.rawValue.capitalized
    case nil: nil
    }
}

private func promptAnnotation(_ prompt: Prompt) -> String {
    var parts = [promptCapabilityLabel(prompt.capability) ?? "Any surface"]
    if !prompt.placeholderNames.isEmpty {
        parts.append("{\(prompt.placeholderNames.joined(separator: "} {"))}")
    }
    return parts.joined(separator: " · ")
}

private struct PromptCard: View {
    let prompt: Prompt
    let onOpen: () -> Void
    let onDelete: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: Design.Space.l) {
                HStack(alignment: .center, spacing: Design.Space.l) {
                    IconPlaque(size: 44) {
                        Image(systemName: promptGlyph(prompt))
                            .font(Design.glyphNav)
                            .foregroundStyle(Design.inkSoft)
                    }
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        Text(prompt.title.isEmpty ? "Untitled" : prompt.title)
                            .font(Design.title)
                            .tracking(Design.tightTracking)
                            .lineLimit(1)
                            .foregroundStyle(Design.ink)
                        Text(scopeLine)
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                    }
                    Spacer(minLength: 0)
                }
                HStack(spacing: Design.Space.s) {
                    TintChip(text: promptCapabilityLabel(prompt.capability) ?? "Any surface")
                    ForEach(prompt.placeholderNames, id: \.self) { name in
                        TintChip(text: "{\(name)}")
                    }
                }
                Text(prompt.body.isEmpty ? "No message yet." : prompt.body)
                    .font(Design.caption)
                    .foregroundStyle(prompt.body.isEmpty ? Design.inkFaint : Design.inkSoft)
                    .lineLimit(2, reservesSpace: true)
                    .multilineTextAlignment(.leading)
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, alignment: .topLeading)
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
        .contextMenu {
            Button("Delete", role: .destructive, action: onDelete)
        }
        .accessibilityLabel(prompt.title.isEmpty ? "Untitled prompt" : prompt.title)
        .accessibilityIdentifier("prompt-card-\(prompt.id)")
    }

    private var scopeLine: String {
        promptCapabilityLabel(prompt.capability).map { "\($0) only" } ?? "Any surface"
    }
}

private func promptGlyph(_ prompt: Prompt) -> String {
    switch prompt.capability {
    case .some(.chat): "message"
    case .some(.image): "photo.stack"
    case .some(.speak): "speaker.wave.2"
    default: "text.quote"
    }
}

private struct NewPromptCard: View {
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(spacing: Design.Space.m) {
                Image(systemName: "plus")
                    .font(Design.glyphNav)
                    .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                Text("New prompt")
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
            }
            .frame(maxWidth: .infinity, minHeight: 132, maxHeight: .infinity)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.tile)
                    .fill(hovering ? Design.inkWash : .clear))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.tile)
                    .strokeBorder(
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                        style: StrokeStyle(lineWidth: Design.hairlineWidth, dash: [5, 4])))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.tile))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.tile))
        .animation(Design.wash, value: hovering)
        .accessibilityLabel("New prompt")
        .accessibilityIdentifier("prompts-new")
    }
}

private struct PromptSheet: View {
    let isNew: Bool
    let onSave: (Prompt) -> Void
    let onDelete: (() -> Void)?
    let onClose: () -> Void
    @State private var draft: Prompt

    init(
        prompt: Prompt, isNew: Bool, onSave: @escaping (Prompt) -> Void,
        onDelete: (() -> Void)?, onClose: @escaping () -> Void
    ) {
        self.isNew = isNew
        self.onSave = onSave
        self.onDelete = onDelete
        self.onClose = onClose
        _draft = State(initialValue: prompt)
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
                    VStack(alignment: .leading, spacing: Design.Space.m) {
                        MicroHeader(title: "Prompt")
                        VStack(alignment: .leading, spacing: Design.Space.m) {
                            InkField(placeholder: "How the / menu lists it", text: $draft.title)
                            InkTextArea(
                                placeholder: "The message to insert",
                                text: $draft.body,
                                resizable: true)
                            Text(
                                "{selection} is replaced with whatever you had drafted before the slash."
                            )
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                            .lineSpacing(1.5)
                        }
                        .padding(Design.Space.tile)
                        .surfaceCard(radius: Design.Radius.tile)
                    }
                    VStack(alignment: .leading, spacing: Design.Space.m) {
                        MicroHeader(title: "Scope")
                        InkRadioGroup(options: scopeOptions, selection: scopeSelection)
                            .padding(Design.Space.m)
                            .surfaceCard(radius: Design.Radius.tile)
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
        .frame(width: Design.Sheet.promptWidth)
        .frame(maxHeight: Design.Sheet.promptHeight)
        .accessibilityIdentifier("prompt-sheet")
    }

    private var scopeOptions: [(value: String, label: String)] {
        [
            (value: "any", label: "Any surface"),
            (value: "chat", label: "Chat"),
            (value: "speak", label: "Speak"),
            (value: "image", label: "Image"),
        ]
    }

    private var scopeSelection: Binding<String> {
        Binding(
            get: { draft.capability?.rawValue ?? "any" },
            set: { value in
                draft.capability = value == "any" ? nil : Capability(rawValue: value)
            })
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: promptGlyph(draft))
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
                    .contentTransition(.symbolEffect(.replace))
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(isNew ? "New prompt" : "Edit prompt")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .lineLimit(1)
                Text("Inserted from any composer with /.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
        .animation(Design.spring, value: draft.capability)
    }

    private var footer: some View {
        HStack(spacing: Design.Space.m) {
            if let onDelete {
                Button("Delete", action: onDelete)
                    .buttonStyle(QuietButtonStyle())
            }
            Spacer()
            Button("Save") {
                onSave(draft)
            }
            .buttonStyle(InkButtonStyle())
            .keyboardShortcut(.defaultAction)
            .disabled(draft.title.isEmpty && draft.body.isEmpty)
            .accessibilityIdentifier("prompt-save")
        }
    }
}
