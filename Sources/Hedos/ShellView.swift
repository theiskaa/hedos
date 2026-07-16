import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ShellModel {
    struct PendingLaunch: Hashable {
        var modelID: String
        var intent: ChatViewModel.Intent
        var seed: String = ""
    }

    let library: LibraryViewModel
    let gallery: GalleryModel
    let settings: SettingsModel
    let audio: AudioSession
    let installs: InstallModel
    let system = SystemMonitor()

    var mode: AppMode = .home
    var chatSelection: String?
    var imagesSelection: String?
    var showingGallery = false
    var commandPaletteOpen = false
    var settingsOpen = false
    var installBrowserOpen = false
    private(set) var settingsDismissAttempts = 0
    var galleryFocusID: String?
    var pendingLaunch: PendingLaunch?
    var voiceSelection: String?
    var pipelineSelection: String?
    var librarySelection: String?
    var sessions: [ChatSession] = []
    var usageByDay: [DayUsage] = []
    private(set) var usageLoaded = false
    var sidebarCollapsed = false
    var isFullscreen = false
    var settingsTarget: SettingsDestination?
    var modelsFilter: ModelFilter?
    var showingGatewayLog = false
    var chatQuery = ""
    var chatSearchFocusTick = 0
    var resident: [Kernel.ResidentEntry] = []
    var residencyBudgetMB = 0
    var preferredChatModelID: String?
    private var residencyTask: Task<Void, Never>?
    private var started = false
    @ObservationIgnored private var chatModels: [String: ChatViewModel] = [:]
    @ObservationIgnored private var chatModelOrder: [String] = []

    var kernel: Kernel { library.kernel }

    func openSettings() {
        settingsOpen = true
        surfaceMainWindow()
    }

    func openSettings(at target: SettingsDestination) {
        settingsTarget = target
        openSettings()
    }

    private func surfaceMainWindow() {
        NSApp.activate(ignoringOtherApps: true)
        guard let window = NSApp.windows.first(where: isMainWindow) else { return }
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(nil)
    }

    private func isMainWindow(_ window: NSWindow) -> Bool {
        window.styleMask.contains(.fullSizeContentView) && !(window is NSPanel)
    }

    func closeSettings() {
        settingsOpen = false
    }

    func requestSettingsDismiss() {
        settingsDismissAttempts += 1
    }

    func handleCloseCommand() {
        guard let key = NSApp.keyWindow else { return }
        guard settingsOpen, isMainWindow(key) else {
            key.performClose(nil)
            return
        }
        if commandPaletteOpen {
            commandPaletteOpen = false
        } else {
            requestSettingsDismiss()
        }
    }

    func chatModel(for session: ChatSession) -> ChatViewModel {
        if let cached = chatModels[session.id] {
            chatModelOrder.removeAll { $0 == session.id }
            chatModelOrder.append(session.id)
            return cached
        }
        let model = ChatViewModel(kernel: kernel, session: session, audio: audio)
        model.onSessionsChanged = { [weak self] in
            Task {
                await self?.refreshSessions()
                await self?.gallery.load()
            }
        }
        model.recordsProvider = { [weak library] in library?.records ?? [] }
        chatModels[session.id] = model
        chatModelOrder.append(session.id)
        trimChatModels()
        return model
    }

    func discardChatModel(_ sessionID: String) {
        chatModels.removeValue(forKey: sessionID)?.stop()
        chatModelOrder.removeAll { $0 == sessionID }
    }

    private func trimChatModels() {
        var overflow = chatModels.count - 8
        guard overflow > 0 else { return }
        for id in chatModelOrder where overflow > 0 {
            guard id != chatSelection, let model = chatModels[id],
                !model.isWorking, !model.isTranscribing, model.draft.isEmpty
            else { continue }
            chatModels.removeValue(forKey: id)
            chatModelOrder.removeAll { $0 == id }
            overflow -= 1
        }
    }

    static let surfaces: [AppMode] = [.chat]

    static func surfaced(_ mode: AppMode) -> AppMode {
        switch mode {
        case .images, .voice, .pipelines: .chat
        default: mode
        }
    }

    init() {
        let library = LibraryViewModel(kernel: Kernel())
        self.library = library
        self.gallery = GalleryModel(kernel: library.kernel)
        self.settings = SettingsModel(kernel: library.kernel)
        self.audio = AudioSession(kernel: library.kernel)
        self.installs = InstallModel(kernel: library.kernel)
        self.settings.audio = audio
        self.installs.recordsProvider = { [weak library] in library?.records ?? [] }
        library.recordsChanged = { [weak installs = self.installs] in
            installs?.reconcileCompleted()
        }
    }

    init(library: LibraryViewModel) {
        self.library = library
        self.gallery = GalleryModel(kernel: library.kernel)
        self.settings = SettingsModel(kernel: library.kernel)
        self.audio = AudioSession(kernel: library.kernel)
        self.installs = InstallModel(kernel: library.kernel)
        self.settings.audio = audio
        self.installs.recordsProvider = { [weak library] in library?.records ?? [] }
        library.recordsChanged = { [weak installs = self.installs] in
            installs?.reconcileCompleted()
        }
    }

    func start() async {
        guard !started else { return }
        started = true
        await settings.load()
        preferredChatModelID = await kernel.settings.defaultChatModelID()
        let restored = await kernel.settings.shellState()
        mode = restored.mode
        chatSelection = restored.chatSessionID
        imagesSelection = restored.imagesSelection
        voiceSelection = restored.voiceModelID
        pipelineSelection = restored.pipelineSelection
        librarySelection = restored.libraryModelID
        sidebarCollapsed = restored.sidebarCollapsed
        if !settings.general.restoreLastSession {
            mode = settings.general.fixedMode ?? .home
        }
        if mode == .settings {
            mode = .home
        }
        mode = Self.surfaced(mode)
        await refreshSessions()
        watchResidency()
        await kernel.setChatConsentAsk { request in
            await ConsentCoordinator.shared.ask(request)
        }
        Task { await kernel.startGatewayIfEnabled() }
        await library.rescan()
        await kernel.startWatching()
        library.startLiveUpdates()
    }

    private func watchResidency() {
        residencyTask?.cancel()
        residencyTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                if self.residencyBudgetMB == 0 {
                    self.residencyBudgetMB = await self.kernel.governor.defaultBudgetMB
                }
                let fresh = await self.kernel.residentModels()
                if fresh != self.resident {
                    self.resident = fresh
                }
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    var residentUsedMB: Int {
        resident.reduce(0) { $0 + $1.footprintMB }
    }

    func selectAdjacentChat(_ offset: Int) {
        if mode != .chat {
            setMode(.chat)
        }
        let list = filteredSessions
        guard !list.isEmpty else { return }
        guard let current = list.firstIndex(where: { $0.id == chatSelection }) else {
            selectChat(offset > 0 ? list.first?.id : list.last?.id)
            return
        }
        let target = min(max(current + offset, 0), list.count - 1)
        selectChat(list[target].id)
    }

    func focusChatSearch() {
        if mode != .chat {
            setMode(.chat)
        }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(80))
            chatSearchFocusTick += 1
        }
    }

    func setSidebarCollapsed(_ collapsed: Bool) {
        guard sidebarCollapsed != collapsed else { return }
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
            sidebarCollapsed = collapsed
        }
        persist()
    }

    func setMode(_ newMode: AppMode) {
        let target = Self.surfaced(newMode)
        guard mode != target, target != .settings else { return }
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
            mode = target
        }
        if target == .chat {
            Task { await refreshSessions() }
        }
        persist()
    }

    func selectChat(_ id: String?) {
        chatSelection = id
        persist()
    }

    func selectImages(_ id: String?) {
        imagesSelection = id
        persist()
    }

    func selectLibrary(_ id: String?) {
        librarySelection = id
        persist()
    }

    func newChat() {
        let kernel = kernel
        Task {
            let preferred = await kernel.settings.defaultChatModelID()
            preferredChatModelID = preferred
            guard let record = Launcher.defaultChatModel(in: library.records, preferring: preferred)
            else { return }
            startChat(bound: record)
        }
    }

    func isWarm(_ record: ModelRecord) -> Bool {
        resident.contains {
            $0.modelID == record.id || ($0.origin == .ollama && $0.name == record.name)
        }
    }

    func showArtifact(_ id: String?) {
        if let id, gallery.artifact(id: id) != nil {
            imagesSelection = id
            galleryFocusID = id
        } else if imagesSelection == nil || gallery.artifact(id: imagesSelection) == nil {
            imagesSelection = gallery.arranged.first?.id
        }
        showingGallery = true
        persist()
    }

    func importChat(from url: URL) {
        let kernel = kernel
        Task {
            do {
                let transcript = try ChatExport.decode(try Data(contentsOf: url))
                let session = try await kernel.chats.importTranscript(transcript)
                await refreshSessions()
                chatSelection = session.id
                mode = .chat
                persist()
            } catch {
                presentError("The chat archive could not be imported.", error)
            }
        }
    }

    private func presentError(_ headline: String, _ error: Error) {
        let alert = NSAlert()
        alert.messageText = headline
        alert.informativeText = error.localizedDescription
        alert.alertStyle = .warning
        alert.runModal()
    }

    var sessionFilter: ChatSessionFilter = .active

    func setSessionFilter(_ filter: ChatSessionFilter) {
        sessionFilter = filter
    }

    var filteredSessions: [ChatSession] {
        sessions.filter { $0.archived == (sessionFilter == .archived) }
    }

    func refreshSessions() async {
        sessions = (try? await kernel.chats.sessions(filter: .all)) ?? sessions
    }

    func refreshUsage() async {
        let since = Calendar.current.date(byAdding: .day, value: -371, to: .now) ?? .now
        usageByDay = await kernel.chatUsage(since: since)
        usageLoaded = true
    }

    func session(id: String?) -> ChatSession? {
        guard let id else { return nil }
        return sessions.first { $0.id == id }
    }

    func launch(_ record: ModelRecord) {
        switch Launcher.destination(for: record) {
        case .chat:
            startChat(bound: record)
        case .images:
            openConversation(with: record, intent: .image)
        case .voice:
            openConversation(with: record, intent: .speak)
        default:
            break
        }
    }

    private func openConversation(with record: ModelRecord, intent: ChatViewModel.Intent) {
        librarySelection = nil
        pendingLaunch = PendingLaunch(modelID: record.id, intent: intent)
        if session(id: chatSelection) != nil {
            mode = .chat
            persist()
            return
        }
        let kernel = kernel
        let records = library.records
        Task {
            let preferred = await kernel.settings.defaultChatModelID()
            let chatModel = Launcher.defaultChatModel(in: records, preferring: preferred)
            let session = try? await kernel.createChatSession(modelID: chatModel?.id)
            await refreshSessions()
            chatSelection = session?.id
            mode = .chat
            persist()
        }
    }

    func startChat(
        bound record: ModelRecord, intent: ChatViewModel.Intent = .text, seed: String = ""
    ) {
        let kernel = kernel
        let records = library.records
        Task {
            let boundID: String?
            switch intent {
            case .text:
                boundID = record.id
            case .image, .speak:
                let preferred = await kernel.settings.defaultChatModelID()
                boundID = Launcher.defaultChatModel(in: records, preferring: preferred)?.id
            }
            guard let session = try? await kernel.createChatSession(modelID: boundID) else {
                return
            }
            await refreshSessions()
            chatSelection = session.id
            librarySelection = nil
            let trimmed = seed.trimmingCharacters(in: .whitespacesAndNewlines)
            if intent != .text || !trimmed.isEmpty {
                pendingLaunch = PendingLaunch(modelID: record.id, intent: intent, seed: trimmed)
            }
            let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                mode = .chat
            }
            persist()
        }
    }

    private func persist() {
        let state = ShellState(
            mode: mode,
            chatSessionID: chatSelection,
            imagesSelection: imagesSelection,
            voiceModelID: voiceSelection,
            pipelineSelection: pipelineSelection,
            libraryModelID: librarySelection,
            sidebarCollapsed: sidebarCollapsed)
        let kernel = kernel
        Task {
            try? await kernel.settings.saveShellState(state)
        }
    }
}

struct ShellView: View {
    @Bindable var shell: ShellModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var nowPlayingClearance: CGFloat {
        shell.isFullscreen ? Design.Space.l : Design.Space.pane + Design.Space.l
    }

    var body: some View {
        HStack(spacing: 0) {
            HedosSidebar(shell: shell)
            Rectangle()
                .fill(Design.line)
                .frame(width: 1)
            pane
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background {
                    Design.paper.ignoresSafeArea()
                }
                .overlay(alignment: .topTrailing) {
                    NowPlayingCard(session: shell.audio)
                        .padding(.top, nowPlayingClearance)
                        .padding(.trailing, Design.Space.l)
                        .animation(
                            Design.motion(reduceMotion: reduceMotion),
                            value: shell.audio.track)
                }
        }
        .ignoresSafeArea(.container, edges: .top)
        .scrollEdgeEffectStyle(.none, for: .top)
        .frame(minWidth: Design.Window.mainMin.width, minHeight: Design.Window.mainMin.height)
        .containerBackground(Design.paper, for: .window)
        .environment(
            \.conversationWidth,
            shell.settings.appearance.chatWidth == .wide
                ? Design.conversationWideWidth : Design.conversationMaxWidth
        )
        .environment(
            \.transcriptSpacing,
            shell.settings.appearance.density == .compact
                ? Design.Space.l : Design.Space.xxl
        )
        .environment(\.chatShowsStats, shell.settings.chat.showStats)
        .environment(\.sendWithEnter, shell.settings.chat.sendWithEnter)
        .tint(Design.ink)
        .id(shell.settings.appearance.themeIdentity)
        .onReceive(
            NotificationCenter.default.publisher(
                for: NSWindow.willEnterFullScreenNotification)
        ) { notification in
            if let window = notification.object as? NSWindow, window == NSApp.mainWindow {
                shell.isFullscreen = true
            }
        }
        .onReceive(
            NotificationCenter.default.publisher(
                for: NSWindow.willExitFullScreenNotification)
        ) { notification in
            if let window = notification.object as? NSWindow, window == NSApp.mainWindow {
                shell.isFullscreen = false
            }
        }
        .task { await shell.start() }
        .settingsOverlay(shell: shell)
        .commandPalette(isPresented: $shell.commandPaletteOpen, shell: shell)
    }

    @ViewBuilder
    private var pane: some View {
        switch shell.mode {
        case .home:
            HomePane(shell: shell)
                .transition(.opacity)
        case .chat, .images, .voice:
            ChatPane(shell: shell)
                .transition(.opacity)
        case .pipelines:
            PipelinesPane(shell: shell)
                .transition(.opacity)
        case .library:
            ModelsPane(shell: shell)
                .transition(.opacity)
        case .gateway:
            GatewayPane(shell: shell)
                .transition(.opacity)
        case .settings:
            ModelsPane(shell: shell)
                .transition(.opacity)
        }
    }
}

struct HedosSidebar: View {
    @Bindable var shell: ShellModel
    @State private var hovered: AppMode?
    @State private var versionHovered = false
    @State private var hoverFraction: CGFloat?
    @State private var versionWidth: CGFloat = 1
    @State private var railQuery = ""
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private func groupTitle(_ title: String) -> some View {
        Text(title)
            .font(Design.label.weight(.semibold))
            .foregroundStyle(Design.inkFaint)
            .padding(.horizontal, Design.Space.l)
            .padding(.top, Design.Space.l)
            .padding(.bottom, Design.Space.xxs)
    }

    var body: some View {
        CollapsingSidebar(collapsed: shell.sidebarCollapsed) {
            expanded
        } collapsedContent: {
            collapsedRail
        }
        .accessibilityIdentifier("shell-rail")
    }

    private var topClearance: CGFloat {
        shell.isFullscreen ? Design.Space.l : Design.Space.pane + Design.Space.l
    }

    private var collapser: some View {
        SidebarCollapseToggle(collapsed: shell.sidebarCollapsed) {
            shell.setSidebarCollapsed(!shell.sidebarCollapsed)
        }
        .accessibilityIdentifier("shell-collapse")
    }

    private var expanded: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Design.Space.s) {
                InkSearchField(placeholder: "Search", query: $railQuery)
                collapser
            }
            .padding(.bottom, Design.Space.l)
            VStack(alignment: .leading, spacing: Design.Space.xs) {
                if rowMatches(Design.modeTitle(.home)) {
                    modeRow(.home, collapsedRow: false)
                }
                if surfacesMatch {
                    groupTitle("Surfaces")
                }
                ForEach(ShellModel.surfaces, id: \.self) { mode in
                    if rowMatches(Design.modeTitle(mode)) {
                        modeRow(mode, collapsedRow: false)
                    }
                }
                if rowMatches(Design.modeTitle(.library))
                    || rowMatches(Design.modeTitle(.gateway))
                {
                    groupTitle("Library")
                    if rowMatches(Design.modeTitle(.library)) {
                        modeRow(.library, collapsedRow: false)
                    }
                    if rowMatches(Design.modeTitle(.gateway)) {
                        modeRow(.gateway, collapsedRow: false)
                    }
                }
            }
            Spacer(minLength: 0)
            settingsRow(collapsedRow: false)
            Rectangle()
                .fill(Design.line.opacity(0.45))
                .frame(height: Design.hairlineWidth)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.xs)
                .accessibilityHidden(true)
            versionRow(collapsedRow: false)
                .padding(.bottom, Design.Space.l)
        }
        .padding(.top, topClearance)
        .padding(.horizontal, Design.Space.l)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private var collapsedRail: some View {
        VStack(alignment: .center, spacing: 0) {
            collapser
                .padding(.bottom, Design.Space.l)
            VStack(alignment: .center, spacing: Design.Space.xs) {
                modeRow(.home, collapsedRow: true)
                ForEach(ShellModel.surfaces, id: \.self) { mode in
                    modeRow(mode, collapsedRow: true)
                }
                Rectangle()
                    .fill(Design.line)
                    .frame(width: 28, height: Design.hairlineWidth)
                    .padding(.vertical, Design.Space.s)
                    .accessibilityHidden(true)
                modeRow(.library, collapsedRow: true)
                modeRow(.gateway, collapsedRow: true)
            }
            Spacer(minLength: 0)
            settingsRow(collapsedRow: true)
            Rectangle()
                .fill(Design.line)
                .frame(width: 28, height: Design.hairlineWidth)
                .padding(.vertical, Design.Space.s)
                .accessibilityHidden(true)
            versionRow(collapsedRow: true)
                .padding(.bottom, Design.Space.l)
        }
        .padding(.top, topClearance)
        .padding(.horizontal, Design.Space.m)
        .frame(maxHeight: .infinity, alignment: .top)
    }

    private func modeRow(_ mode: AppMode, collapsedRow: Bool) -> some View {
        InkSidebarRow(
            id: mode,
            glyph: Design.modeGlyph(mode),
            title: Design.modeTitle(mode),
            selected: shell.mode == mode,
            collapsed: collapsedRow,
            hovered: $hovered
        ) {
            railQuery = ""
            shell.setMode(mode)
        }
        .help(
            shortcutNumber(mode).map { "\(Design.modeTitle(mode)) — ⌘\($0)" }
                ?? Design.modeTitle(mode))
        .accessibilityIdentifier("rail-\(mode.rawValue)")
    }

    private func shortcutNumber(_ mode: AppMode) -> Int? {
        let visible = AppMode.allCases.filter {
            $0 != .settings && ShellModel.surfaced($0) == $0
        }
        return visible.firstIndex(of: mode).map { $0 + 1 }
    }

    private func rowMatches(_ title: String) -> Bool {
        railQuery.isEmpty || title.localizedCaseInsensitiveContains(railQuery)
    }

    private var surfacesMatch: Bool {
        ShellModel.surfaces.contains {
            rowMatches(Design.modeTitle($0))
        }
    }

    @ViewBuilder
    private func versionRow(collapsedRow: Bool) -> some View {
        let updateAvailable = Updater.shared.available != nil
        let interactive = updateAvailable || !Updater.shared.isUnversioned
        let lit = versionHovered && interactive
        let wash = Color.clear
        let help =
            updateAvailable
            ? "Update available — click to install"
            : interactive
                ? "Hedos \(Updater.shared.displayVersion) — check for updates"
                : "Development build"
        let announce =
            updateAvailable
            ? "Update available, Hedos \(Updater.shared.displayVersion)"
            : "Hedos \(Updater.shared.displayVersion)"
        let tracked = versionContent(
            collapsedRow: collapsedRow, lit: lit, wash: wash, updateAvailable: updateAvailable
        )
        .background(
            GeometryReader { proxy in
                Color.clear
                    .onAppear { versionWidth = proxy.size.width }
                    .onChange(of: proxy.size.width) { _, value in versionWidth = value }
            }
        )
        .onContinuousHover(coordinateSpace: .local) { phase in
            switch phase {
            case .active(let point):
                versionHovered = true
                hoverFraction = min(max(point.x / max(versionWidth, 1), 0), 1)
            case .ended:
                versionHovered = false
                hoverFraction = nil
            }
        }
        if interactive {
            Button {
                if updateAvailable {
                    Updater.shared.installAvailable()
                } else {
                    Updater.shared.checkFromMenu()
                }
            } label: {
                tracked
            }
            .buttonStyle(PressDipStyle())
            .animation(Design.wash, value: lit)
            .inkFocusRing(RoundedRectangle.soft(Design.Radius.control))
            .help(help)
            .accessibilityLabel(announce)
            .accessibilityIdentifier("rail-version")
        } else {
            tracked
                .help(help)
                .accessibilityLabel(announce)
                .accessibilityIdentifier("rail-version")
        }
    }

    @ViewBuilder
    private func versionContent(
        collapsedRow: Bool, lit: Bool, wash: Color, updateAvailable: Bool
    ) -> some View {
        if collapsedRow {
            HedosGlowLogo(size: 20, fraction: hoverFraction, lit: lit)
                .frame(width: 44, height: 36)
                .background(RoundedRectangle.soft(Design.Radius.control).fill(wash))
                .overlay(alignment: .topTrailing) {
                    if updateAvailable {
                        Circle()
                            .fill(Color.green)
                            .frame(width: 7, height: 7)
                            .padding(.top, 6)
                            .padding(.trailing, 6)
                    }
                }
                .contentShape(RoundedRectangle.soft(Design.Radius.control))
        } else {
            HStack(spacing: Design.Space.chipX) {
                HedosGlowLogo(size: 22, fraction: hoverFraction, lit: lit)
                    .frame(width: 22, alignment: .leading)
                VStack(alignment: .leading, spacing: 1) {
                    Text("hedos")
                        .font(Design.body)
                        .fontWeight(.semibold)
                        .foregroundStyle(lit ? Design.ink : Design.inkSoft)
                    Text("version: \(cleanVersion)")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
                Spacer(minLength: 0)
                if updateAvailable {
                    Circle().fill(Color.green).frame(width: 7, height: 7)
                }
            }
            .padding(.horizontal, Design.Space.l)
            .padding(.vertical, Design.Space.s + 1)
            .background(RoundedRectangle.soft(Design.Radius.control).fill(wash))
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
    }

    private var cleanVersion: String {
        let raw = Updater.shared.displayVersion
        return raw.hasPrefix("v") ? String(raw.dropFirst()) : raw
    }

    private func settingsRow(collapsedRow: Bool) -> some View {
        InkSidebarRow(
            id: AppMode.settings,
            glyph: Design.modeGlyph(.settings),
            title: Design.modeTitle(.settings),
            selected: false,
            collapsed: collapsedRow,
            hovered: $hovered
        ) {
            shell.openSettings()
        }
        .help("Settings — ⌘,")
        .accessibilityIdentifier("rail-settings")
    }
}

struct ColumnDivider: View {
    var body: some View {
        Rectangle()
            .fill(Design.hairline)
            .frame(width: Design.hairlineWidth)
    }
}

struct ChatPane: View {
    @Bindable var shell: ShellModel
    @State private var galleryViewing: Artifact?
    @State private var galleryDeleting: Artifact?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ChatSessionsColumn(shell: shell, query: $shell.chatQuery)
                    .frame(width: Design.Rail.columnWidth)
                ColumnDivider()
                detail
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .animation(Design.wash, value: shell.chatSelection)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .task { await shell.gallery.load() }
        .modalScrim(
            isPresented: shell.showingGallery, onDismiss: { shell.showingGallery = false }
        ) {
            GallerySheet(shell: shell, viewing: $galleryViewing, deleting: $galleryDeleting) {
                shell.showingGallery = false
            }
        }
        .overlay(alignment: .center) {
            if let artifact = galleryViewing {
                GalleryImageViewer(
                    shell: shell,
                    artifact: artifact,
                    onClose: { galleryViewing = nil },
                    onDelete: { galleryDeleting = artifact })
                    .transition(.opacity)
            }
        }
        .animation(Design.wash, value: galleryViewing?.id)
        .confirmationDialog(
            "Move this image to the Trash?",
            isPresented: Binding(
                get: { galleryDeleting != nil },
                set: { if !$0 { galleryDeleting = nil } })
        ) {
            Button("Move to Trash", role: .destructive) {
                if let artifact = galleryDeleting {
                    let shell = shell
                    Task {
                        await shell.gallery.delete(artifact)
                        if shell.imagesSelection == artifact.id {
                            shell.selectImages(nil)
                        }
                        if galleryViewing?.id == artifact.id {
                            galleryViewing = nil
                        }
                    }
                }
                galleryDeleting = nil
            }
        } message: {
            Text("The file moves to the Trash, not deleted outright.")
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let session = shell.session(id: shell.chatSelection) {
            ChatView(
                session: session, model: shell.chatModel(for: session),
                library: shell.library, kernel: shell.kernel,
                audio: shell.audio,
                launch: shell.pendingLaunch,
                onOpenArtifacts: { [weak shell] reference in
                    shell?.showArtifact(reference)
                },
                onNewChat: { [weak shell] in
                    shell?.newChat()
                },
                onLaunchConsumed: { [weak shell] in
                    shell?.pendingLaunch = nil
                }
            )
            .id(session.id)
            .transition(.opacity)
        } else {
            ChatStartHero(shell: shell)
                .transition(.opacity)
        }
    }
}

struct ModeEmptyState<Extra: View>: View {
    var glyph: String?
    var eyebrow: String?
    let headline: String
    let caption: String
    @ViewBuilder let extra: () -> Extra

    init(
        glyph: String? = nil, eyebrow: String? = nil, headline: String, caption: String,
        @ViewBuilder extra: @escaping () -> Extra = { EmptyView() }
    ) {
        self.glyph = glyph
        self.eyebrow = eyebrow
        self.headline = headline
        self.caption = caption
        self.extra = extra
    }

    var body: some View {
        VStack(spacing: Design.Space.l) {
            if let glyph {
                Image(systemName: glyph)
                    .font(.system(size: 22, weight: .regular))
                    .foregroundStyle(Design.inkSoft)
                    .frame(width: 60, height: 60)
                    .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
                    .overlay(
                        RoundedRectangle.soft(Design.Radius.card)
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                    .shade(Design.Elevation.raised)
                    .staggeredArrival(0)
            }
            VStack(spacing: Design.Space.m) {
                if let eyebrow {
                    Text(eyebrow.uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                        .staggeredArrival(1)
                }
                Text(headline)
                    .font(Design.markdownHeading(1))
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                    .multilineTextAlignment(.center)
                    .staggeredArrival(2)
                Text(caption)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .lineSpacing(2.5)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: Design.Column.emptyCaption)
                    .staggeredArrival(3)
            }
            extra()
                .padding(.top, Design.Space.s)
                .staggeredArrival(4)
        }
        .padding(Design.Space.pane)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
