import AppKit
import HedosKernel
import SwiftUI

struct SettingsEntry: Identifiable {
    let id: String
    let section: String
    let title: String
    let keywords: [String]

    func matches(_ query: String) -> Bool {
        rank(query) != nil
    }

    func rank(_ query: String) -> Int? {
        let needle = query.lowercased()
        let name = title.lowercased()
        if name.hasPrefix(needle) { return 0 }
        if name.contains(needle) { return 1 }
        if section.lowercased().contains(needle) { return 2 }
        if keywords.contains(where: { $0.lowercased().contains(needle) }) { return 3 }
        return nil
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
            id: "models.servers", section: "Models", title: "Servers",
            keywords: ["endpoint", "openai", "api", "server", "remote", "url", "key"]),
        .init(
            id: "models.runtimes", section: "Models", title: "Installed runtimes",
            keywords: [
                "runtime", "manifest", "community", "install", "vm", "container", "sandbox",
                "contained",
            ]),
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
            id: "appearance.family", section: "Appearance", title: "Theme",
            keywords: ["theme", "palette", "default", "gruvbox", "color"]),
        .init(
            id: "appearance.theme", section: "Appearance", title: "Appearance",
            keywords: ["dark", "light", "system", "mode", "appearance"]),
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
            id: "gateway.enable", section: "Gateway", title: "Serve models over HTTP",
            keywords: ["gateway", "server", "http", "api", "serve", "openai", "ollama", "port"]),
        .init(
            id: "gateway.port", section: "Gateway", title: "Port",
            keywords: ["gateway", "port", "http", "address", "localhost"]),
        .init(
            id: "gateway.endpoints", section: "Gateway", title: "Endpoints",
            keywords: [
                "endpoints", "api", "curl", "connect", "examples", "sdk", "openai", "ollama",
                "routes",
            ]),
        .init(
            id: "gateway.clients", section: "Gateway", title: "Client tokens",
            keywords: ["token", "client", "key", "scope", "auth", "revoke", "bearer"]),
        .init(
            id: "gateway.audit", section: "Gateway", title: "Recent activity",
            keywords: ["audit", "log", "activity", "requests", "history"]),
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
    private var previewTask: Task<Void, Never>?
    var audio: AudioSession?
    var saveNotice: String?

    var general = GeneralSettings()
    var models = ModelsSettings()
    var chat = ChatSettings()
    var voice = VoiceSettings()
    var appearance = AppearanceSettings()
    var advanced = AdvancedSettings()
    var gateway = GatewaySettings()
    var gatewayStatus = GatewayStatus(running: false)
    var gatewayBusy = false
    var gatewayClients: [GatewayClient] = []
    var gatewayAuditEntries: [GatewayAuditEntry] = []
    var gatewayNotice: String?
    var installedRuntimes: [RuntimeManifest] = []
    var prompts: [Prompt] = []
    var voices: [String] = []
    var previewing = false
    var previewingVoice: String?
    var voiceNotice: String?
    private(set) var loaded = false

    static weak var active: SettingsModel?

    init(kernel: Kernel) {
        self.kernel = kernel
        Self.active = self
    }

    func load() async {
        general = await kernel.settings.general()
        models = await kernel.settings.models()
        chat = await kernel.settings.chat()
        voice = await kernel.settings.voice()
        appearance = await kernel.settings.appearance()
        ThemeBootstrap.reconcile(&appearance)
        advanced = await kernel.settings.advanced()
        gateway = await kernel.settings.gateway()
        prompts = await kernel.promptStore.list()
        await refreshGateway()
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
            _ = try await kernel.promptStore.save(value)
        }
    }

    func deletePrompt(_ prompt: Prompt) {
        saveTasks["prompt-\(prompt.id)"]?.cancel()
        prompts.removeAll { $0.id == prompt.id }
        let kernel = kernel
        let id = prompt.id
        Task {
            await kernel.promptStore.delete(id: id)
        }
    }

    func applyTheme() {
        ThemeStore.select(appearance.family)
        let resolved = appearance.theme.nsAppearance
        NSApp.appearance = resolved
        for window in NSApp.windows {
            window.appearance = resolved
        }
        Design.fontBook = Design.FontBook(
            uiFamily: appearance.uiFont, monoFamily: appearance.monoFont)
        ThemeBootstrap.remember(family: appearance.family, mode: appearance.theme)
    }

    func applyShellIntegrations() {
        HotkeyCenter.shared.apply(general.quickAskHotkey)
        MenuBarController.shared.apply(general.menuBarItem)
    }

    func loadVoices(from records: [ModelRecord]) async {
        guard let speaker = SpeechModels.preferred(in: records) else {
            voices = []
            return
        }
        voices = (try? await kernel.voices(for: speaker.id)) ?? []
    }

    func saveGeneral() {
        applyShellIntegrations()
        let value = general
        persist("general") { kernel in
            try await kernel.settings.save(value)
        }
    }

    func saveModels() {
        let value = models
        persist("models") { kernel in
            try await kernel.settings.save(value)
        }
    }

    func saveChat() {
        let value = chat
        persist("chat") { kernel in
            try await kernel.settings.save(value)
        }
    }

    func saveVoice() {
        let value = voice
        persist("voice") { kernel in
            try await kernel.settings.save(value)
        }
    }

    func saveAppearance() {
        applyTheme()
        let value = appearance
        let kernel = kernel
        saveTasks["appearance"]?.cancel()
        saveTasks["appearance"] = Task {
            do {
                try await kernel.settings.save(value)
                saveNotice = nil
            } catch is CancellationError {
            } catch {
                saveNotice = "Couldn't save this change: \(error.localizedDescription)"
            }
        }
    }

    func saveAdvanced() {
        let value = advanced
        persist("advanced") { kernel in
            try await kernel.settings.save(value)
        }
    }

    func refreshGateway() async {
        gatewayStatus = await kernel.gatewayStatus()
        gatewayClients = await kernel.gatewayClientStore.list()
        gatewayAuditEntries = await kernel.gatewayAuditLog.tail(limit: 20).reversed()
    }

    func setGatewayEnabled(_ enabled: Bool) {
        guard !gatewayBusy else { return }
        gatewayBusy = true
        gateway.enabled = enabled
        gatewayNotice = nil
        let value = gateway
        let kernel = kernel
        Task {
            defer { gatewayBusy = false }
            try await kernel.settings.save(value)
            if enabled {
                do {
                    _ = try await kernel.startGateway()
                } catch {
                    self.gatewayNotice =
                        (error as? GatewayError)?.message ?? error.localizedDescription
                }
            } else {
                await kernel.stopGateway()
            }
            await self.refreshGateway()
        }
    }

    func applyGatewayPort() {
        gatewayNotice = nil
        let value = gateway
        let kernel = kernel
        Task {
            try await kernel.settings.save(value)
            if await kernel.gatewayStatus().running {
                await kernel.stopGateway()
                do {
                    _ = try await kernel.startGateway()
                } catch {
                    self.gatewayNotice =
                        (error as? GatewayError)?.message ?? error.localizedDescription
                }
            }
            await self.refreshGateway()
        }
    }

    func createGatewayClient(
        name: String, scopes: GatewayScopes
    ) async -> GatewayClientCreation? {
        let creation = try? await kernel.gatewayClientStore.create(name: name, scopes: scopes)
        await refreshGateway()
        return creation
    }

    func revokeGatewayClient(id: String) {
        let kernel = kernel
        Task {
            try? await kernel.gatewayClientStore.revoke(id: id)
            await self.refreshGateway()
        }
    }

    var gatewayAuditFileURL: URL {
        kernel.gatewayAuditLog.logURL
    }

    func refreshInstalledRuntimes() async {
        installedRuntimes = kernel.runtimeCatalog.installedCommunity()
    }

    func previewRuntimeInstall(from url: URL) async throws -> RuntimeInstallPreview {
        try await kernel.previewRuntimeInstall(from: url)
    }

    func installRuntime(from url: URL) async throws {
        _ = try await kernel.installRuntime(from: url)
        await refreshInstalledRuntimes()
    }

    func uninstallRuntime(id: String) {
        let kernel = kernel
        Task {
            try? await kernel.uninstallRuntime(id: id)
            await self.refreshInstalledRuntimes()
        }
    }

    func previewVoice(records: [ModelRecord], named candidate: String? = nil) {
        let chosen = candidate ?? voice.defaultVoice ?? voices.first ?? ""
        if previewing {
            stopVoicePreview()
            guard previewingVoice != chosen else {
                previewingVoice = nil
                return
            }
        }
        guard !chosen.isEmpty, let speaker = SpeechModels.preferred(in: records) else { return }
        previewing = true
        previewingVoice = chosen
        let kernel = kernel
        let liveID = "preview-\(chosen)"
        audio?.beginLive(
            AudioSession.Track(id: liveID, title: SpeechModels.previewLine, subtitle: chosen),
            audible: true,
            onStop: { [weak self] in self?.stopVoicePreview() })
        previewTask = Task { [weak self] in
            defer {
                self?.previewing = false
                self?.previewingVoice = nil
            }
            guard let self else { return }
            do {
                let stream = try await kernel.invoke(
                    speaker.id, .speak,
                    payload: .object([
                        "text": .string(SpeechModels.previewLine),
                        "voice": .string(chosen),
                    ]))
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        self.audio?.enqueue(frame, for: liveID)
                    }
                }
                self.voiceNotice = nil
            } catch is CancellationError {
            } catch {
                self.voiceNotice = error.localizedDescription
            }
            self.audio?.finishLive(liveID)
        }
    }

    func stopVoicePreview() {
        previewTask?.cancel()
        let candidate = previewingVoice
        previewing = false
        previewingVoice = nil
        if let candidate, audio?.isActive("preview-\(candidate)") == true {
            audio?.dismiss()
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
            _ = try? await kernel.promptStore.save(prompt)
        }
        try? await kernel.settings.save(general)
        try? await kernel.settings.save(models)
        try? await kernel.settings.save(chat)
        try? await kernel.settings.save(voice)
        try? await kernel.settings.save(appearance)
        try? await kernel.settings.save(advanced)
        try? await kernel.settings.save(gateway)
    }

    private func persist(_ key: String, _ operation: @escaping (Kernel) async throws -> Void) {
        saveTasks[key]?.cancel()
        let kernel = kernel
        saveTasks[key] = Task {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            do {
                try await operation(kernel)
                saveNotice = nil
            } catch is CancellationError {
            } catch {
                saveNotice = "Couldn't save this change: \(error.localizedDescription)"
            }
        }
    }
}

struct SettingRow<Control: View>: View {
    let id: String
    let label: String
    var caption: String? = nil
    let highlighted: Bool
    @ViewBuilder let control: () -> Control

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
            RoundedRectangle.soft(Design.Radius.control)
                .fill(highlighted ? Design.ink.opacity(0.08) : .clear)
                .padding(.vertical, Design.Space.xs))
        .padding(.horizontal, -Design.Space.s)
        .animation(Design.wash, value: highlighted)
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
    case gateway
    case advanced

    var title: String {
        switch self {
        case .general: "General"
        case .models: "Models"
        case .prompts: "Prompts"
        case .chat: "Chat"
        case .voice: "Voice"
        case .appearance: "Appearance"
        case .gateway: "Gateway"
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
        case .gateway: "network"
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
        case .gateway: "Serve your models to local tools, with tokens."
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

extension AppearanceSettings {
    var themeIdentity: String {
        "\(Design.fontBook.identity)/\(family)"
    }
}

enum ThemeBootstrap {
    private static let familyKey = "hedos.appearance.family"
    private static let modeKey = "hedos.appearance.mode"

    static func remember(family: String, mode: AppearanceSettings.Theme) {
        UserDefaults.standard.set(family, forKey: familyKey)
        UserDefaults.standard.set(mode.rawValue, forKey: modeKey)
    }

    @MainActor
    static func apply() {
        let family = UserDefaults.standard.string(forKey: familyKey) ?? ThemeFamily.defaultID
        ThemeStore.select(family)
        if let raw = UserDefaults.standard.string(forKey: modeKey),
            let mode = AppearanceSettings.Theme(rawValue: raw)
        {
            NSApp.appearance = mode.nsAppearance
        }
    }

    static func reconcile(_ appearance: inout AppearanceSettings) {
        if let family = UserDefaults.standard.string(forKey: familyKey) {
            appearance.family = family
        }
        if let raw = UserDefaults.standard.string(forKey: modeKey),
            let mode = AppearanceSettings.Theme(rawValue: raw)
        {
            appearance.theme = mode
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
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .ignoresSafeArea(.container, edges: .top)
        .scrollEdgeEffectStyle(.none, for: .top)
        .background(Design.paper.ignoresSafeArea())
        .id(model.appearance.themeIdentity)
        .modalScrim(
            isPresented: showingAddServer,
            onDismiss: { showingAddServer = false }
        ) {
            AddServerSheet(shell: shell) {
                showingAddServer = false
            }
        }
        .modalScrim(
            isPresented: showingAddGatewayClient,
            onDismiss: { showingAddGatewayClient = false }
        ) {
            AddGatewayClientSheet(shell: shell) {
                showingAddGatewayClient = false
            }
        }
        .modalScrim(
            isPresented: showingGatewayConnect,
            onDismiss: { showingGatewayConnect = false }
        ) {
            GatewayConnectSheet(shell: shell) {
                showingGatewayConnect = false
            }
        }
        .modalScrim(
            isPresented: installCandidate != nil,
            onDismiss: { installCandidate = nil }
        ) {
            if let source = installCandidate {
                InstallRuntimeSheet(shell: shell, source: source) {
                    installCandidate = nil
                }
            }
        }
        .modalScrim(
            isPresented: promptDraft != nil,
            onDismiss: { promptDismissAttempts += 1 }
        ) {
            if let draft = promptDraft {
                PromptSheet(
                    prompt: draft,
                    isNew: promptDraftIsNew,
                    dismissAttempts: promptDismissAttempts,
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
        }
        .task(id: shell.library.shelfSignature) {
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
                    expandedGroup("System", [.gateway, .advanced])
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
                    collapsedGroup([.gateway, .advanced])
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
    @State private var showingAddServer = false
    @State private var showingAddGatewayClient = false
    @State private var showingGatewayConnect = false
    @State private var installCandidate: URL?
    @State private var promptDraftIsNew = false
    @State private var promptDismissAttempts = 0

    private var detail: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xxl) {
                    if let notice = model.saveNotice {
                        HStack(spacing: Design.Space.s) {
                            Image(systemName: "exclamationmark.triangle")
                                .font(Design.glyphInline)
                                .foregroundStyle(Design.heat)
                            Text(notice)
                                .font(Design.caption.weight(.medium))
                                .foregroundStyle(Design.inkSoft)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .transition(.arrive(from: .top))
                    }
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
                        Group {
                            switch selected {
                            case .general: generalSection
                            case .models: modelsSection
                            case .chat: chatSection
                            case .voice: voiceSection
                            case .appearance: appearanceSection
                            case .prompts: promptsSection
                            case .gateway:
                                GatewaySection(
                                    shell: shell, highlighted: highlighted,
                                    onAddClient: { showingAddGatewayClient = true },
                                    onConnect: { showingGatewayConnect = true })
                            case .advanced: advancedSection
                            }
                        }
                        .id(selected)
                        .transition(.opacity)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.pane)
                .padding(.bottom, Design.Space.xxl)
                .animation(Design.motion(reduceMotion: reduceMotion), value: selected)
                .animation(Design.wash, value: model.saveNotice)
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
        let matches = SettingsIndex.entries
            .compactMap { entry in entry.rank(query).map { (entry, $0) } }
            .sorted { $0.1 < $1.1 }
            .map(\.0)
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
            RowRule()
            settingRow("general.startMode", "Start in") {
                InkDropdown(
                    options: AppMode.allCases.filter {
                        $0 != .settings && ShellModel.surfaced($0) == $0
                    }
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
                RowRule()
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
                RowRule()
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
                RowRule()
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
                RowRule()
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
                        if model.models.ramBudgetMB != nil {
                            Button {
                                model.models.ramBudgetMB = nil
                                model.saveModels()
                            } label: {
                                Text("auto")
                                    .font(Design.label)
                                    .foregroundStyle(Design.inkFaint)
                            }
                            .buttonStyle(PressDipStyle())
                            .transition(.arrive(from: .trailing))
                            .accessibilityLabel("Reset RAM budget to auto")
                        }
                    }
                    .frame(width: Design.Column.control)
                    .animation(Design.wash, value: model.models.ramBudgetMB == nil)
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
            group("Hugging Face caches") {
                hfCacheRows
            }
            group("Servers") {
                serverRows
            }
            group("Installed runtimes") {
                runtimeRows
            }
        }
    }

    private var runtimeRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            ForEach(shell.settings.installedRuntimes, id: \.id) { manifest in
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "shippingbox")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                    Text(manifest.id)
                        .font(Design.label)
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                    Text("contained")
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    Spacer()
                    ConfirmableIconButton(
                        label: "Uninstall \(manifest.id)", confirmLabel: "Remove?"
                    ) {
                        shell.settings.uninstallRuntime(id: manifest.id)
                    }
                    .fixedSize()
                }
            }
            if shell.settings.installedRuntimes.isEmpty {
                Text("Community runtimes run inside their own Linux machine — never loose on the Mac.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Button("Install a runtime…") {
                RuntimePicker.pick { url in
                    installCandidate = url
                }
            }
            .buttonStyle(QuietButtonStyle())
            .accessibilityIdentifier("runtimes-install")
            .padding(.top, Design.Space.xs)
        }
        .padding(.vertical, Design.Space.m)
        .id("models.runtimes")
        .background(highlightBackground("models.runtimes"))
        .task { await shell.settings.refreshInstalledRuntimes() }
    }

    private var serverRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            ForEach(shell.library.endpointRecords, id: \.id) { record in
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: .endpoint, size: 14)
                        .foregroundStyle(Design.inkSoft)
                    Text(record.displayName)
                        .font(Design.label)
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                    Text(record.source.path)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    ConfirmableIconButton(
                        label: "Remove \(record.displayName)", confirmLabel: "Remove?"
                    ) {
                        let shell = shell
                        let id = record.id
                        Task { await shell.library.removeEndpoint(id: id) }
                    }
                    .fixedSize()
                }
            }
            if shell.library.endpointRecords.isEmpty {
                Text("Any OpenAI-compatible local server — llama-server, LM Studio, vLLM.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Button("Add a server…") {
                showingAddServer = true
            }
            .buttonStyle(QuietButtonStyle())
            .padding(.top, Design.Space.xs)
        }
        .padding(.vertical, Design.Space.m)
        .id("models.servers")
        .background(highlightBackground("models.servers"))
    }

    private var warmRows: some View {
        ResidencyStrip(shell: shell)
    }

    private var budgetBar: some View {
        BudgetBar(shell: shell)
    }

    private var hfCacheRows: some View {
        FolderListSection(
            folders: shell.library.hfCacheRoots,
            emptyText: "Standard locations and HF_HOME are always scanned.",
            onRemove: { path in
                let shell = shell
                Task { await shell.library.removeHFRoot(path) }
            },
            onAdd: { url in
                let shell = shell
                Task { await shell.library.addHFRoot(url) }
            })
            .padding(.vertical, Design.Space.m)
            .id("models.hfCache")
            .background(highlightBackground("models.hfCache"))
    }

    private var foldersRows: some View {
        FolderListSection(
            folders: shell.library.watchedFolders,
            emptyText: "Standard locations are always scanned.",
            onRemove: { path in
                let shell = shell
                Task { await shell.library.removeFolder(path) }
            },
            onAdd: { url in
                let shell = shell
                Task { await shell.library.addFolder(url) }
            })
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
                RowRule()
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
                RowRule()
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
                        onPreview: { candidate in
                            model.previewVoice(
                                records: shell.library.records, named: candidate)
                        },
                        onSelect: { choice in
                            model.voice.defaultVoice = choice
                            model.saveVoice()
                            if let choice {
                                model.previewVoice(
                                    records: shell.library.records, named: choice)
                            }
                        })
                    if model.previewing {
                        SpeakingIndicator()
                    }
                    Button(model.previewing ? "Stop" : "Preview") {
                        if model.previewing {
                            model.stopVoicePreview()
                        } else {
                            model.previewVoice(records: shell.library.records)
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(model.voices.isEmpty)
                    .help(
                        model.voices.isEmpty
                            ? "Voices appear when a speech model is ready." : "")
                }
                if let voiceNotice = model.voiceNotice {
                    Text(voiceNotice)
                        .font(Design.label)
                        .foregroundStyle(Design.heatText)
                        .lineLimit(2)
                }
            }
            RowRule()
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
            RowRule()
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
            group("Appearance") {
                HStack(alignment: .center) {
                    Text("Mode")
                        .font(Design.caption.weight(.medium))
                        .foregroundStyle(Design.ink)
                    Spacer(minLength: Design.Space.l)
                    AppearanceModeToggle(
                        selection: model.appearance.theme,
                        onSelect: { mode in
                            model.appearance.theme = mode
                            model.saveAppearance()
                        })
                }
                .padding(.vertical, Design.Space.l)
                .padding(.horizontal, Design.Space.s)
                .id("appearance.theme")
                .background(highlightBackground("appearance.theme"))
            }
            group("Theme") {
                LazyVGrid(
                    columns: [
                        GridItem(.flexible(), spacing: Design.Space.m),
                        GridItem(.flexible(), spacing: Design.Space.m),
                    ],
                    spacing: Design.Space.m
                ) {
                    ForEach(ThemeFamily.all) { family in
                        ThemeFamilyCard(
                            family: family,
                            selected: model.appearance.family == family.id,
                            action: {
                                model.appearance.family = family.id
                                model.saveAppearance()
                            })
                    }
                }
                .padding(.vertical, Design.Space.chipX)
                .id("appearance.family")
                .background(highlightBackground("appearance.family"))
            }
            group("Type") {
                settingRow("appearance.fontUI", "App font") {
                    InkDropdown(
                        options: FontCatalog.uiFamilies,
                        selection: model.appearance.uiFont,
                        placeholder: "San Francisco",
                        accessibilityName: "app font",
                        width: 220,
                        rowFont: { .custom($0, size: 12) },
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
                        width: 220,
                        rowFont: { .custom($0, size: 12) },
                        onSelect: { family in
                            model.appearance.monoFont = family
                            model.saveAppearance()
                        })
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
        _ header: String, @ViewBuilder content: @escaping () -> some View
    ) -> some View {
        SettingsGroup(header: header, content: content)
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
        RoundedRectangle.soft(Design.Radius.card)
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
        let overBudget = usedMB > budgetMB
        GeometryReader { proxy in
            let width = proxy.size.width
            ZStack(alignment: .leading) {
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(Design.line)
                    .frame(height: 8)
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(overBudget ? Design.heat : Design.accent)
                    .frame(
                        width: min(max(0, width * CGFloat(usedMB) / CGFloat(totalMB)), width),
                        height: 8)
                Rectangle()
                    .fill(Design.ink)
                    .frame(width: 2, height: 11)
                    .offset(
                        x: min(width * CGFloat(budgetMB) / CGFloat(totalMB), width) - 1)
            }
            .animation(Design.spring, value: usedMB)
            .animation(Design.spring, value: budgetMB)
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
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: Design.Space.s) {
                    Text(verbatim: "▸")
                        .font(Design.body.weight(.semibold))
                        .foregroundStyle(Design.accent)
                        .help("Appears in the / menu")
                    Text(prompt.title.isEmpty ? "Untitled" : prompt.title)
                        .font(Design.title)
                        .lineLimit(1)
                        .foregroundStyle(Design.ink)
                    Spacer(minLength: Design.Space.m)
                    Text((promptCapabilityLabel(prompt.capability) ?? "any").uppercased())
                        .font(Design.label)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
                .padding(.horizontal, Design.Space.tile)
                .padding(.vertical, Design.Space.l)
                Rectangle()
                    .fill(Design.line)
                    .frame(height: Design.hairlineWidth)
                Text(prompt.body.isEmpty ? "No message yet." : prompt.body)
                    .font(Design.readingBody)
                    .lineSpacing(2)
                    .foregroundStyle(prompt.body.isEmpty ? Design.inkFaint : Design.inkSoft)
                    .lineLimit(3, reservesSpace: true)
                    .multilineTextAlignment(.leading)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(Design.Space.tile)
                    .background(Design.panel)
                if !tags.isEmpty {
                    HStack(spacing: Design.Space.l) {
                        ForEach(tags, id: \.self) { tag in
                            Text("#\(tag)")
                                .font(Design.micro)
                                .foregroundStyle(Design.inkFaint)
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, Design.Space.tile)
                    .padding(.vertical, Design.Space.m)
                }
            }
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
            .clipShape(RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
            .lifts(hovering: hovering)
        }
        .buttonStyle(.plain)
        .overlay(alignment: .topTrailing) {
            if hovering {
                ConfirmableIconButton(
                    glyph: "trash",
                    label: "Delete \(prompt.title.isEmpty ? "prompt" : prompt.title)",
                    confirmLabel: "Delete?"
                ) {
                    onDelete()
                }
                .padding(Design.Space.m)
                .transition(.opacity)
            }
        }
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .contextMenu {
            Button("Delete", role: .destructive, action: onDelete)
        }
        .accessibilityLabel(prompt.title.isEmpty ? "Untitled prompt" : prompt.title)
        .accessibilityIdentifier("prompt-card-\(prompt.id)")
    }

    private var tags: [String] {
        var result: [String] = []
        if let capability = promptCapabilityLabel(prompt.capability) {
            result.append(capability.lowercased())
        }
        result.append(contentsOf: prompt.placeholderNames)
        return result
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
                RoundedRectangle.soft(Design.Radius.tile)
                    .fill(hovering ? Design.inkWash : .clear))
            .overlay(
                RoundedRectangle.soft(Design.Radius.tile)
                    .strokeBorder(
                        hovering ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                        style: StrokeStyle(lineWidth: Design.hairlineWidth, dash: [5, 4])))
            .contentShape(RoundedRectangle.soft(Design.Radius.tile))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle.soft(Design.Radius.tile))
        .animation(Design.wash, value: hovering)
        .accessibilityLabel("New prompt")
        .accessibilityIdentifier("prompts-new")
    }
}

private struct PromptSheet: View {
    let isNew: Bool
    let dismissAttempts: Int
    let onSave: (Prompt) -> Void
    let onDelete: (() -> Void)?
    let onClose: () -> Void
    private let original: Prompt
    @State private var draft: Prompt
    @State private var confirmingDiscard = false

    init(
        prompt: Prompt, isNew: Bool, dismissAttempts: Int,
        onSave: @escaping (Prompt) -> Void,
        onDelete: (() -> Void)?, onClose: @escaping () -> Void
    ) {
        self.isNew = isNew
        self.dismissAttempts = dismissAttempts
        self.onSave = onSave
        self.onDelete = onDelete
        self.onClose = onClose
        self.original = prompt
        _draft = State(initialValue: prompt)
    }

    private var isDirty: Bool { draft != original }

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
        .onChange(of: dismissAttempts) {
            if isDirty {
                confirmingDiscard = true
            } else {
                onClose()
            }
        }
        .onChange(of: draft) {
            if confirmingDiscard { confirmingDiscard = false }
        }
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
            if confirmingDiscard {
                Button("Discard changes") { onClose() }
                    .buttonStyle(PressDipStyle())
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.heatText)
                    .transition(.arrive(from: .leading))
            } else if let onDelete {
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
        .animation(Design.wash, value: confirmingDiscard)
    }
}

private struct FolderListSection: View {
    let folders: [String]
    let emptyText: String
    let onRemove: (String) -> Void
    let onAdd: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            ForEach(folders, id: \.self) { path in
                FolderRow(path: path) { onRemove(path) }
            }
            if folders.isEmpty {
                Text(emptyText)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Button("Add…") {
                FolderRow.pickFolder { onAdd($0) }
            }
            .buttonStyle(QuietButtonStyle())
            .padding(.top, Design.Space.xs)
        }
    }
}
