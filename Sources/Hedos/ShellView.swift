import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ShellModel {
    struct PendingLaunch: Hashable {
        var modelID: String
        var intent: ChatViewModel.Intent
    }

    let library: LibraryViewModel
    let gallery: GalleryModel
    let settings: SettingsModel
    let audio: AudioSession
    let system = SystemMonitor()

    var mode: AppMode = .home
    var chatSelection: String?
    var imagesSelection: String?
    var showingGallery = false
    var galleryFocusID: String?
    var pendingLaunch: PendingLaunch?
    var voiceSelection: String?
    var pipelineSelection: String?
    var librarySelection: String?
    var sessions: [ChatSession] = []
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
        self.settings.audio = audio
    }

    init(library: LibraryViewModel) {
        self.library = library
        self.gallery = GalleryModel(kernel: library.kernel)
        self.settings = SettingsModel(kernel: library.kernel)
        self.audio = AudioSession(kernel: library.kernel)
        self.settings.audio = audio
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
            switch intent {
            case .text:
                if !trimmed.isEmpty {
                    let chat = chatModel(for: session)
                    chat.draft = trimmed
                    chat.send()
                }
            case .image, .speak:
                pendingLaunch = PendingLaunch(modelID: record.id, intent: intent)
                if !trimmed.isEmpty {
                    chatModel(for: session).draft = trimmed
                }
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
                            Design.motion(
                                reduceMotion: NSWorkspace.shared
                                    .accessibilityDisplayShouldReduceMotion),
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
        .help("\(Design.modeTitle(mode)) — ⌘\(mode.ordinal)")
        .accessibilityIdentifier("rail-\(mode.rawValue)")
    }

    private func rowMatches(_ title: String) -> Bool {
        railQuery.isEmpty || title.localizedCaseInsensitiveContains(railQuery)
    }

    private var surfacesMatch: Bool {
        ShellModel.surfaces.contains {
            rowMatches(Design.modeTitle($0))
        }
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
            SettingsWindowController.shared.show(shell: shell)
        }
        .help("Settings — ⌘,")
        .accessibilityIdentifier("rail-settings")
    }
}

struct SearchSnippet: View {
    let snippet: String

    var body: some View {
        text
    }

    private var text: Text {
        var result = Text(verbatim: "")
        var current = ""
        var marked = false
        for character in snippet {
            switch character {
            case "[" where !marked:
                result = Text("\(result)\(Text(current))")
                current = ""
                marked = true
            case "]" where marked:
                result = Text("\(result)\(highlighted(current))")
                current = ""
                marked = false
            default:
                current.append(character)
            }
        }
        if !current.isEmpty {
            result = Text("\(result)\(marked ? highlighted(current) : Text(current))")
        }
        return result
    }

    private func highlighted(_ value: String) -> Text {
        Text(value).fontWeight(.semibold).foregroundStyle(Design.inkSoft)
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
        } else {
            ChatStartHero(shell: shell)
        }
    }
}

struct ChatStartHero: View {
    @Bindable var shell: ShellModel
    @State private var draft = ""
    @State private var selectedModelID: String?
    @FocusState private var fieldFocused: Bool

    private var defaultModel: ModelRecord? {
        Launcher.defaultChatModel(
            in: shell.library.records, preferring: shell.preferredChatModelID)
    }

    private var startableGroups: [(section: String, records: [ModelRecord])] {
        let ready = shell.library.records.filter {
            $0.state == .ready
                && [.chat, .images, .voice].contains(Launcher.destination(for: $0))
        }
        return [(AppMode.chat, "Chat"), (.images, "Image"), (.voice, "Voice")]
            .compactMap { mode, label in
                let records = ready.filter { Launcher.destination(for: $0) == mode }
                return records.isEmpty ? nil : (section: label, records: records)
            }
    }

    private var activeModel: ModelRecord? {
        if let id = selectedModelID,
            let picked = shell.library.records.first(where: { $0.id == id })
        {
            return picked
        }
        return defaultModel
    }

    private var activeIntent: ChatViewModel.Intent {
        guard let model = activeModel else { return .text }
        switch Launcher.destination(for: model) {
        case .images: return .image
        case .voice: return .speak
        default: return .text
        }
    }

    var body: some View {
        VStack(spacing: Design.Space.l) {
            VStack(spacing: Design.Space.s) {
                Text("Every conversation starts here.")
                    .font(Design.hero)
                    .foregroundStyle(Design.ink)
                    .multilineTextAlignment(.center)
                Text(subtitle)
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkSoft)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 440)
            }
            if !startableGroups.isEmpty {
                modelPicker
            }
            startChatField
            Text(caption)
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .multilineTextAlignment(.center)
            Button("Import a conversation…") {
                let panel = NSOpenPanel()
                panel.allowedContentTypes = [.json]
                panel.allowsMultipleSelection = false
                if panel.runModal() == .OK, let url = panel.url {
                    shell.importChat(from: url)
                }
            }
            .buttonStyle(.plain)
            .font(Design.label)
            .foregroundStyle(Design.inkSoft)
            .padding(.top, Design.Space.s)
        }
        .frame(maxWidth: 540)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(Design.Space.pane)
    }

    private var modelPicker: some View {
        InkMenu(
            title: activeModel?.displayName ?? "Choose model",
            accessibilityName: "Start model",
            readyDot: activeModel != nil ? true : nil,
            trigger: .chip
        ) {
            ForEach(startableGroups, id: \.section) { group in
                InkMenuHeader(title: group.section)
                ForEach(group.records) { record in
                    InkMenuRow(
                        title: record.displayName,
                        selected: record.id == activeModel?.id
                    ) {
                        selectedModelID = record.id
                    }
                }
            }
        }
    }

    private var startChatField: some View {
        HStack(alignment: .center, spacing: Design.Space.m) {
            TextField(fieldPlaceholder, text: $draft)
                .textFieldStyle(.plain)
                .font(Design.body)
                .focused($fieldFocused)
                .onSubmit(launchChat)
                .disabled(activeModel == nil)
            Button(action: launchChat) {
                Image(systemName: "arrow.up")
                    .font(Design.caption.weight(.semibold))
                    .foregroundStyle(Design.paper)
                    .frame(width: 28, height: 28)
                    .background(Design.ink, in: Circle())
            }
            .buttonStyle(PressDipStyle())
            .disabled(sendDisabled)
            .accessibilityLabel("Start chat")
        }
        .padding(.leading, Design.Space.xl)
        .padding(.trailing, Design.Space.s)
        .padding(.vertical, Design.Space.s)
        .surfaceCard(radius: Design.Radius.bubble)
        .onTapGesture { fieldFocused = true }
    }

    private var sendDisabled: Bool {
        activeModel == nil
            || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var fieldPlaceholder: String {
        switch activeIntent {
        case .image: return "Describe an image…"
        case .speak: return "Type something to speak…"
        case .text: return "Start a chat…"
        }
    }

    private var subtitle: String {
        if activeModel != nil {
            return "Ask anything. It runs on your machine and stays there."
        }
        if defaultModel == nil && startableGroups.isEmpty {
            return "When a chat-capable model lands on your shelf, it opens here."
        }
        return "Pick a model to begin. It runs on your machine and stays there."
    }

    private var caption: String {
        guard let model = activeModel else {
            return "No chat-capable model is ready."
        }
        let kind: String
        switch activeIntent {
        case .image: kind = "image"
        case .speak: kind = "voice"
        case .text: kind = "chat"
        }
        return "New \(kind) chats use \(model.displayName) · runs local, stays private"
    }

    private func launchChat() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let record = activeModel else { return }
        shell.startChat(bound: record, intent: activeIntent, seed: text)
        draft = ""
    }
}

struct ChatSessionsColumn: View {
    @Bindable var shell: ShellModel
    @Binding var query: String
    @State private var renaming: ChatSession?
    @State private var renameTitle = ""
    @State private var deleting: ChatSession?
    @State private var hits: [SearchHit] = []
    @State private var hoveredSession: String?
    @State private var hoveredHit: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: Design.Space.s) {
                InkSearchField(
                    placeholder: "Search chats", query: $query, fill: Design.surface,
                    focusTick: shell.chatSearchFocusTick)
                if !shell.gallery.arranged.isEmpty {
                    QuietIconButton(glyph: "square.grid.2x2") {
                        shell.showingGallery = true
                    }
                    .help("All generated images")
                    .accessibilityLabel("Gallery")
                }
                QuietIconButton(
                    glyph: "archivebox",
                    fill: shell.sessionFilter == .archived
                ) {
                    shell.setSessionFilter(
                        shell.sessionFilter == .active ? .archived : .active)
                }
                .help(
                    shell.sessionFilter == .archived
                        ? "Showing archived chats" : "Show archived chats")
                .accessibilityLabel("Filter conversations")
                QuietIconButton(glyph: "square.and.pencil") {
                    shell.newChat()
                }
                .disabled(Launcher.defaultChatModel(in: shell.library.records) == nil)
                .help(
                    Launcher.defaultChatModel(in: shell.library.records) == nil
                        ? "No chat-capable model is ready yet" : "New chat")
                .accessibilityLabel("New chat")
            }
            .padding(.horizontal, Design.Space.m)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.s)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: Design.Space.xxs) {
                    if !query.isEmpty {
                        let matches = shell.filteredSessions.filter {
                            $0.title.localizedCaseInsensitiveContains(query)
                        }
                        if matches.isEmpty {
                            Text("Nothing found.")
                                .font(Design.caption)
                                .foregroundStyle(Design.inkFaint)
                                .padding(Design.Space.m)
                        } else {
                            ForEach(matches) { session in
                                ChatSessionRow(
                                    session: session, shell: shell,
                                    hovered: $hoveredSession
                                )
                                .contextMenu { rowActions(session) }
                            }
                        }
                    } else if shell.filteredSessions.isEmpty {
                        Text(
                            shell.sessionFilter == .archived
                                ? "Nothing archived."
                                : "No conversations yet."
                        )
                        .font(Design.caption)
                        .foregroundStyle(Design.inkFaint)
                        .padding(Design.Space.m)
                    } else {
                        ForEach(SessionGrouping.groups(shell.filteredSessions), id: \.title) { group in
                            MicroHeader(title: group.title)
                                .padding(.horizontal, Design.Space.chipX)
                                .padding(.top, Design.Space.l)
                                .padding(.bottom, Design.Space.xxs)
                            ForEach(group.sessions) { session in
                                ChatSessionRow(
                                    session: session, shell: shell,
                                    hovered: $hoveredSession
                                )
                                .contextMenu { rowActions(session) }
                            }
                        }
                    }
                }
                .padding(.horizontal, Design.Space.m)
                .padding(.top, Design.Space.s)
                .padding(.bottom, Design.Space.l)
                .animation(
                    Design.motion(
                        reduceMotion: NSWorkspace.shared
                            .accessibilityDisplayShouldReduceMotion),
                    value: shell.filteredSessions.map(\.id))
            }
        }
        .task { await shell.refreshSessions() }
        .alert(
            "Rename Chat",
            isPresented: Binding(
                get: { renaming != nil },
                set: { if !$0 { renaming = nil } })
        ) {
            TextField("Title", text: $renameTitle)
            Button("Rename") {
                if let session = renaming {
                    rename(session, to: renameTitle)
                }
                renaming = nil
            }
            Button("Cancel", role: .cancel) {
                renaming = nil
            }
        }
        .confirmationDialog(
            "Delete “\(deleting?.title ?? "")”?",
            isPresented: Binding(
                get: { deleting != nil },
                set: { if !$0 { deleting = nil } })
        ) {
            Button("Delete", role: .destructive) {
                if let session = deleting {
                    delete(session)
                }
                deleting = nil
            }
            .keyboardShortcut(.defaultAction)
        } message: {
            Text("The conversation leaves your history.")
        }
        .onDeleteCommand {
            if let id = shell.chatSelection,
                let session = shell.sessions.first(where: { $0.id == id })
            {
                deleting = session
            }
        }
    }

    @ViewBuilder
    private func rowActions(_ session: ChatSession) -> some View {
        let live = shell.session(id: session.id) ?? session
        Button("Rename…") {
            renameTitle = live.title
            renaming = live
        }
        Button(live.pinned ? "Unpin" : "Pin") {
            setPinned(live, !live.pinned)
        }
        Button(live.archived ? "Unarchive" : "Archive") {
            setArchived(live, !live.archived)
        }
        Divider()
        if shell.settings.chat.exportFormat == .json {
            Button("Export as JSON…") {
                export(session, json: true)
            }
            Button("Export as Markdown…") {
                export(session, json: false)
            }
        } else {
            Button("Export as Markdown…") {
                export(session, json: false)
            }
            Button("Export as JSON…") {
                export(session, json: true)
            }
        }
        Divider()
        Button("Delete…", role: .destructive) {
            deleting = session
        }
    }

    private func export(_ session: ChatSession, json: Bool) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = session.title + (json ? ".json" : ".md")
        let shell = shell
        panel.begin { response in
            guard response == .OK, let url = panel.url else { return }
            Task {
                guard let transcript = try? await shell.kernel.chats.session(id: session.id)
                else { return }
                if json {
                    try? ChatExport.json(transcript).write(to: url)
                } else {
                    try? Data(ChatExport.markdown(transcript).utf8).write(to: url)
                }
            }
        }
    }

    private func rename(_ session: ChatSession, to title: String) {
        let trimmed = title.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        mutate { try await $0.renameSession(id: session.id, title: trimmed) }
    }

    private func setPinned(_ session: ChatSession, _ pinned: Bool) {
        mutate { try await $0.setPinned(id: session.id, pinned) }
    }

    private func setArchived(_ session: ChatSession, _ archived: Bool) {
        mutate { try await $0.setArchived(id: session.id, archived) }
        if archived, shell.chatSelection == session.id {
            shell.selectChat(nil)
        }
    }

    private func delete(_ session: ChatSession) {
        let shell = shell
        Task {
            do {
                try await shell.kernel.chats.deleteSession(id: session.id)
                shell.discardChatModel(session.id)
                if shell.chatSelection == session.id {
                    shell.selectChat(nil)
                }
            } catch {}
            await shell.refreshSessions()
        }
    }

    private func mutate(_ change: @escaping @Sendable (ChatStore) async throws -> Void) {
        let shell = shell
        Task {
            try? await change(shell.kernel.chats)
            await shell.refreshSessions()
        }
    }
}

private struct ChatSessionRow: View {
    let session: ChatSession
    let shell: ShellModel
    @Binding var hovered: String?

    private var selected: Bool { shell.chatSelection == session.id }
    private var hovering: Bool { hovered == session.id }

    var body: some View {
        Button {
            shell.selectChat(session.id)
        } label: {
            HStack(spacing: Design.Space.s) {
                VStack(alignment: .leading, spacing: Design.Space.xxs) {
                    Text(session.title)
                        .font(Design.body.weight(selected ? .semibold : .medium))
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                    Text(subtitle)
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                        .lineLimit(1)
                }
                Spacer(minLength: Design.Space.xs)
                HStack(spacing: Design.Space.xs) {
                    ForEach(session.capabilityTags.compactMap(Design.tagGlyph), id: \.self) {
                        glyph in
                        Image(systemName: glyph)
                            .font(Design.glyphSmall)
                            .foregroundStyle(Design.inkFaint)
                    }
                    if session.pinned {
                        Image(systemName: "pin.fill")
                            .font(Design.glyphMicro)
                            .foregroundStyle(Design.inkFaint)
                    }
                }
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.m)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected
                    ? Design.ink.opacity(0.08)
                    : hovering ? Design.ink.opacity(0.04) : .clear,
                in: RoundedRectangle.soft(Design.Radius.control))
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside {
                hovered = session.id
            } else if hovered == session.id {
                hovered = nil
            }
        }
        .accessibilityLabel(session.title)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityIdentifier("session-\(session.id)")
    }

    private var subtitle: String {
        var parts: [String] = []
        if let modelID = session.modelID {
            parts.append(shell.library.record(id: modelID)?.displayName ?? modelID)
        }
        parts.append(session.updatedAt.formatted(.relative(presentation: .named)))
        return parts.joined(separator: " · ")
    }

}

struct GallerySheet: View {
    @Bindable var shell: ShellModel
    @Binding var viewing: Artifact?
    @Binding var deleting: Artifact?
    let onClose: () -> Void
    @State private var hoveredCell: String?

    private let columns = [GridItem(.adaptive(minimum: 168), spacing: Design.Space.tile)]

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(
                title: "Gallery",
                subtitle: headerSubtitle,
                onClose: onClose,
                plaque: {
                    Image(systemName: "photo.stack")
                        .font(Design.glyphPrimary)
                        .foregroundStyle(Design.inkSoft)
                })
            if shell.gallery.arranged.isEmpty {
                emptyState
            } else {
                grid
            }
        }
        .frame(width: Design.Sheet.gallery.width, height: Design.Sheet.gallery.height)
        .task {
            await shell.gallery.load()
            if let id = shell.galleryFocusID, let artifact = shell.gallery.artifact(id: id) {
                viewing = artifact
            }
            shell.galleryFocusID = nil
        }
    }

    private var grid: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Design.Space.pane) {
                ForEach(sections) { section in
                    VStack(alignment: .leading, spacing: Design.Space.l) {
                        HStack(spacing: Design.Space.m) {
                            MicroHeader(title: section.title)
                            Text("\(section.items.count)")
                                .font(Design.label.weight(.medium))
                                .foregroundStyle(Design.inkFaint)
                            Spacer(minLength: 0)
                        }
                        LazyVGrid(columns: columns, spacing: Design.Space.tile) {
                            ForEach(section.items) { artifact in
                                cell(artifact)
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xs)
            .padding(.bottom, Design.Space.gutter)
        }
    }

    private var countLine: String {
        let count = shell.gallery.arranged.count
        return count == 1 ? "1 image" : "\(count) images"
    }

    private var headerSubtitle: String {
        guard !shell.gallery.arranged.isEmpty else { return "Everything you've made, here" }
        return "\(countLine) · everything you've made"
    }

    private struct Section: Identifiable {
        let id: String
        let title: String
        let items: [Artifact]
    }

    private var sections: [Section] {
        let calendar = Calendar.current
        let startNow = calendar.startOfDay(for: Date())
        var today: [Artifact] = []
        var week: [Artifact] = []
        var earlier: [Artifact] = []
        for artifact in shell.gallery.arranged {
            let start = calendar.startOfDay(for: artifact.createdAt)
            let days = calendar.dateComponents([.day], from: start, to: startNow).day ?? 0
            if days <= 0 {
                today.append(artifact)
            } else if days < 7 {
                week.append(artifact)
            } else {
                earlier.append(artifact)
            }
        }
        var result: [Section] = []
        if !today.isEmpty { result.append(Section(id: "today", title: "Today", items: today)) }
        if !week.isEmpty { result.append(Section(id: "week", title: "This Week", items: week)) }
        if !earlier.isEmpty {
            result.append(Section(id: "earlier", title: "Earlier", items: earlier))
        }
        return result
    }

    private var emptyState: some View {
        VStack(spacing: Design.Space.l) {
            IconPlaque(size: 56) {
                Image(systemName: "photo.on.rectangle.angled")
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkFaint)
            }
            Text("No images yet")
                .font(Design.title)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
            Text("Everything you generate lands here, ready to save or revisit.")
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(2.5)
                .multilineTextAlignment(.center)
                .frame(maxWidth: Design.Column.emptyCaption)
        }
        .padding(Design.Space.pane)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func cell(_ artifact: Artifact) -> some View {
        let hovered = hoveredCell == artifact.id
        return Button {
            viewing = artifact
        } label: {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                thumbnail(artifact)
                caption(artifact)
            }
            .padding(Design.Space.m)
            .frame(maxWidth: .infinity)
            .tile(hovering: hovered)
        }
        .buttonStyle(.plain)
        .overlay(alignment: .topTrailing) {
            if hovered {
                actions(artifact)
                    .padding(Design.Space.m + Design.Space.xxs)
                    .transition(.opacity)
            }
        }
        .onHover { inside in
            if inside {
                hoveredCell = artifact.id
            } else if hoveredCell == artifact.id {
                hoveredCell = nil
            }
        }
        .animation(Design.wash, value: hoveredCell)
        .task(id: artifact.id) {
            await shell.gallery.loadThumbnail(artifact)
        }
        .contextMenu {
            Button("Open") {
                viewing = artifact
            }
            Button("Download…") {
                shell.gallery.download(artifact)
            }
            Divider()
            Button("Delete…", role: .destructive) {
                deleting = artifact
            }
        }
        .accessibilityLabel(Provenance.prompt(of: artifact.params) ?? "Untitled image")
    }

    private func thumbnail(_ artifact: Artifact) -> some View {
        Color.clear
            .aspectRatio(1, contentMode: .fit)
            .frame(maxWidth: .infinity)
            .overlay {
                if let image = shell.gallery.thumbnail(artifact) {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                } else {
                    placeholder(artifact)
                }
            }
            .clipShape(RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
    }

    private func placeholder(_ artifact: Artifact) -> some View {
        ZStack {
            Rectangle().fill(Design.cardFill)
            if artifact.capability == .image {
                SkeletonPulse()
            } else {
                Image(systemName: typeGlyph(artifact))
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkFaint)
            }
        }
    }

    private func caption(_ artifact: Artifact) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                .font(Design.body.weight(.medium))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: Design.Space.m) {
                Text(artifact.createdAt.formatted(.relative(presentation: .named)))
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                Spacer(minLength: Design.Space.xs)
                TintChip(text: typeLabel(artifact), glyph: typeGlyph(artifact), faint: true)
            }
        }
        .padding(.horizontal, Design.Space.xs)
        .padding(.bottom, Design.Space.xxs)
    }

    private func actions(_ artifact: Artifact) -> some View {
        HStack(spacing: Design.Space.xxs) {
            GalleryQuickAction(glyph: "arrow.up.backward.and.arrow.down.forward", label: "Open") {
                viewing = artifact
            }
            GalleryQuickAction(glyph: "arrow.down.to.line", label: "Save") {
                shell.gallery.download(artifact)
            }
            GalleryQuickAction(glyph: "trash", label: "Delete", destructive: true) {
                deleting = artifact
            }
        }
        .padding(Design.Space.xxs + 1)
        .background(Design.paper.opacity(0.94), in: RoundedRectangle.soft(Design.Radius.control))
        .overlay(
            RoundedRectangle.soft(Design.Radius.control)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .shade(Design.Elevation.button)
    }

    private func typeLabel(_ artifact: Artifact) -> String {
        switch artifact.capability {
        case .image: "Image"
        case .speak: "Voice"
        case .transcribe: "Audio"
        default: artifact.capability.rawValue.capitalized
        }
    }

    private func typeGlyph(_ artifact: Artifact) -> String {
        switch artifact.capability {
        case .image: "photo"
        case .speak, .transcribe: "waveform"
        default: "shippingbox"
        }
    }
}

private struct GalleryImageViewer: View {
    @Bindable var shell: ShellModel
    let artifact: Artifact
    let onClose: () -> Void
    let onDelete: () -> Void
    @State private var image: NSImage?
    @State private var loadFailed = false

    var body: some View {
        ZStack {
            Design.shadowColor.opacity(0.72)
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture(perform: onClose)
                .accessibilityLabel("Dismiss")
            VStack(spacing: Design.Space.l) {
                imageStage
                footer
            }
            .padding(Design.Space.xl)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .overlay(alignment: .topTrailing) {
            SheetCloseButton(action: onClose)
                .padding(Design.Space.l)
        }
        .onExitCommand(perform: onClose)
        .task(id: artifact.id) {
            loadFailed = false
            image = shell.gallery.thumbnail(artifact)
            image = await shell.gallery.fullImage(artifact) ?? image
            loadFailed = image == nil
        }
    }

    private var imageStage: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
                    .contentShape(Rectangle())
                    .onTapGesture {}
            } else if loadFailed {
                VStack(spacing: Design.Space.m) {
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(Design.glyphPrimary)
                        .foregroundStyle(Design.inkSoft)
                    Text("Couldn't load this image.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                }
                .frame(width: 240, height: 180)
                .background(Design.cardFill, in: RoundedRectangle.soft(Design.Radius.artifact))
            } else {
                RoundedRectangle.soft(Design.Radius.artifact)
                    .fill(Design.cardFill)
                    .overlay(SkeletonPulse())
                    .frame(width: 240, height: 180)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var footer: some View {
        VStack(spacing: Design.Space.m) {
            Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                .font(Design.body.weight(.medium))
                .foregroundStyle(Design.ink)
                .multilineTextAlignment(.center)
                .lineLimit(2)
                .frame(maxWidth: Design.Bubble.promptMax)
            HStack(spacing: Design.Space.s) {
                GalleryQuickAction(glyph: "arrow.down.to.line", label: "Save") {
                    shell.gallery.download(artifact)
                }
                GalleryQuickAction(
                    glyph: "trash", label: "Delete", destructive: true, action: onDelete)
            }
        }
    }
}

private struct GalleryQuickAction: View {
    let glyph: String
    let label: String
    var destructive = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: glyph)
                .font(Design.glyphSmall)
                .foregroundStyle(
                    destructive && hovering
                        ? Design.danger : hovering ? Design.ink : Design.inkSoft
                )
                .frame(width: 26, height: 26)
                .background(
                    hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Color.clear),
                    in: RoundedRectangle.soft(Design.Radius.control))
                .contentShape(RoundedRectangle.soft(Design.Radius.control))
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}

struct ModeEmptyState<Extra: View>: View {
    var eyebrow: String?
    let headline: String
    let caption: String
    @ViewBuilder let extra: () -> Extra

    init(
        eyebrow: String? = nil, headline: String, caption: String,
        @ViewBuilder extra: @escaping () -> Extra = { EmptyView() }
    ) {
        self.eyebrow = eyebrow
        self.headline = headline
        self.caption = caption
        self.extra = extra
    }

    var body: some View {
        VStack(spacing: Design.Space.l) {
            if let eyebrow {
                Text(eyebrow)
                    .font(Design.label.weight(.medium))
                    .foregroundStyle(Design.inkFaint)
            }
            Text(headline)
                .font(Design.markdownHeading(1))
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
                .multilineTextAlignment(.center)
            Text(caption)
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(2.5)
                .multilineTextAlignment(.center)
                .frame(maxWidth: Design.Column.emptyCaption)
            extra()
                .padding(.top, Design.Space.l)
        }
        .padding(Design.Space.pane)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
