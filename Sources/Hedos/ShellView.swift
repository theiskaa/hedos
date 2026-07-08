import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ShellModel {
    let library: LibraryViewModel
    let images: ImagesViewModel
    let voice: VoiceSurfaceModel
    let settings: SettingsModel

    var mode: AppMode = .library
    var chatSelection: String?
    var imagesSelection: String?
    var pendingImageReveal: String?
    var voiceSelection: String?
    var pipelineSelection: String?
    var librarySelection: String?
    var sessions: [ChatSession] = []
    var sidebarCollapsed = false
    var isFullscreen = false
    var settingsTarget: SettingsDestination?
    var chatQuery = ""
    var chatSearchFocusTick = 0
    var resident: [Kernel.ResidentEntry] = []
    var residencyBudgetMB = 0
    private var residencyTask: Task<Void, Never>?
    private var started = false

    var kernel: Kernel { library.kernel }

    init() {
        let library = LibraryViewModel(kernel: Kernel())
        self.library = library
        self.images = ImagesViewModel(kernel: library.kernel)
        self.voice = VoiceSurfaceModel(kernel: library.kernel)
        self.settings = SettingsModel(kernel: library.kernel)
    }

    init(library: LibraryViewModel) {
        self.library = library
        self.images = ImagesViewModel(kernel: library.kernel)
        self.voice = VoiceSurfaceModel(kernel: library.kernel)
        self.settings = SettingsModel(kernel: library.kernel)
    }

    func start() async {
        guard !started else { return }
        started = true
        await settings.load()
        if let restored = try? await kernel.shellState() {
            mode = restored.mode
            chatSelection = restored.chatSessionID
            imagesSelection = restored.imagesSelection
            voiceSelection = restored.voiceModelID
            pipelineSelection = restored.pipelineSelection
            librarySelection = restored.libraryModelID
            sidebarCollapsed = restored.sidebarCollapsed
        }
        if !settings.general.restoreLastSession {
            mode = settings.general.fixedMode ?? .home
        }
        if mode == .settings {
            mode = .home
        }
        await refreshSessions()
        watchResidency()
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
        guard mode != newMode, newMode != .settings else { return }
        let reduceMotion = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        withAnimation(Design.motion(reduceMotion: reduceMotion)) {
            mode = newMode
        }
        if newMode == .chat {
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

    func selectVoice(_ id: String?) {
        voiceSelection = id
        persist()
    }

    func selectLibrary(_ id: String?) {
        librarySelection = id
        persist()
    }

    func newChat() {
        let kernel = kernel
        Task {
            let preferred = (try? await kernel.defaultChatModelID()) ?? nil
            guard let record = Launcher.defaultChatModel(in: library.records, preferring: preferred)
            else { return }
            startChat(bound: record)
        }
    }

    func showArtifact(_ id: String?) {
        if let id, images.artifact(id: id) != nil {
            imagesSelection = id
            pendingImageReveal = id
        } else if imagesSelection == nil || images.artifact(id: imagesSelection) == nil {
            imagesSelection = images.arranged.first?.id
        }
        mode = .images
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
            images.bind(to: record)
            imagesSelection = nil
            librarySelection = nil
            mode = .images
            persist()
        case .voice:
            voiceSelection = record.id
            librarySelection = nil
            mode = .voice
            let voice = voice
            Task { await voice.bind(to: record) }
            persist()
        default:
            break
        }
    }

    func startChat(bound record: ModelRecord) {
        let kernel = kernel
        Task {
            let session = try? await kernel.chats.createSession(modelID: record.id)
            await refreshSessions()
            chatSelection = session?.id
            librarySelection = nil
            mode = .chat
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
            try? await kernel.saveShellState(state)
        }
    }
}

struct ShellView: View {
    @Bindable var shell: ShellModel

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
        }
        .id(Design.fontBook.identity)
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
        case .chat:
            ChatPane(shell: shell)
                .transition(.opacity)
        case .images:
            ImagesPane(shell: shell)
                .transition(.opacity)
        case .voice:
            VoicePane(shell: shell)
                .transition(.opacity)
        case .pipelines:
            PipelinesPane(shell: shell)
                .transition(.opacity)
        case .library:
            ModelsPane(shell: shell)
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
        Text(title.uppercased())
            .font(Design.micro)
            .tracking(Design.microTracking)
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
                ForEach([AppMode.chat, .images, .voice, .pipelines], id: \.self) { mode in
                    if rowMatches(Design.modeTitle(mode)) {
                        modeRow(mode, collapsedRow: false)
                    }
                }
                if rowMatches(Design.modeTitle(.library)) {
                    groupTitle("Library")
                    modeRow(.library, collapsedRow: false)
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
                modeRow(.chat, collapsedRow: true)
                modeRow(.images, collapsedRow: true)
                modeRow(.voice, collapsedRow: true)
                modeRow(.pipelines, collapsedRow: true)
                Rectangle()
                    .fill(Design.line)
                    .frame(width: 28, height: Design.hairlineWidth)
                    .padding(.vertical, Design.Space.s)
                    .accessibilityHidden(true)
                modeRow(.library, collapsedRow: true)
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
        [AppMode.chat, .images, .voice, .pipelines].contains {
            rowMatches(Design.modeTitle($0))
        }
    }

    private func settingsRow(collapsedRow: Bool) -> some View {
        BrandRow(
            collapsed: collapsedRow,
            hovered: $hovered
        ) {
            SettingsWindowController.shared.show(shell: shell)
        }
        .help("Settings — ⌘,")
        .accessibilityIdentifier("rail-settings")
    }
}

private struct BrandRow: View {
    var collapsed: Bool = false
    @Binding var hovered: AppMode?
    let action: () -> Void

    @State private var hoverPoint: CGPoint?

    var body: some View {
        Button(action: action) {
            if collapsed {
                LogoMark(size: 30)
                    .frame(width: 44, height: 44)
                    .modifier(InkGlow(point: hoverPoint))
                    .modifier(GlowTracking(point: $hoverPoint, hovered: $hovered))
                    .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
            } else {
                HStack(spacing: Design.Space.chipX) {
                    LogoMark(size: 28)
                        .frame(width: 30, alignment: .leading)
                    Text("Hedos")
                        .font(Design.paneTitle)
                        .tracking(Design.tightTracking)
                        .foregroundStyle(Design.ink)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.s + 1)
                .modifier(InkGlow(point: hoverPoint))
                .modifier(GlowTracking(point: $hoverPoint, hovered: $hovered))
                .contentShape(Capsule())
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Settings")
    }
}

private struct GlowTracking: ViewModifier {
    @Binding var point: CGPoint?
    @Binding var hovered: AppMode?

    func body(content: Content) -> some View {
        content.onContinuousHover(coordinateSpace: .local) { phase in
            switch phase {
            case .active(let location):
                point = location
                hovered = .settings
            case .ended:
                point = nil
                if hovered == .settings {
                    hovered = nil
                }
            }
        }
    }
}

private struct InkGlow: ViewModifier {
    let point: CGPoint?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content.overlay {
            if let point {
                GeometryReader { geometry in
                    RadialGradient(
                        colors: [Design.paper.opacity(0.9), .clear],
                        center: UnitPoint(
                            x: point.x / max(geometry.size.width, 1),
                            y: point.y / max(geometry.size.height, 1)),
                        startRadius: 0,
                        endRadius: 52)
                }
                .animation(
                    reduceMotion ? nil : .spring(response: 0.35, dampingFraction: 0.65),
                    value: point)
                .mask(content)
                .allowsHitTesting(false)
            }
        }
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
    }

    @ViewBuilder
    private var detail: some View {
        if let session = shell.session(id: shell.chatSelection) {
            ChatView(
                session: session, library: shell.library, kernel: shell.kernel,
                onSessionsChanged: { [weak shell] in
                    Task { await shell?.refreshSessions() }
                },
                onOpenArtifacts: { [weak shell] reference in
                    shell?.showArtifact(reference)
                },
                onNewChat: { [weak shell] in
                    shell?.newChat()
                },
                onNarrate: { [weak shell] text, turnID in
                    guard let shell else { return }
                    let sessionID = session.id
                    shell.setMode(.voice)
                    Task {
                        await shell.voice.narrate(
                            text, records: shell.library.records,
                            preferring: shell.voiceSelection,
                            attach: (sessionID: sessionID, turnID: turnID))
                    }
                }
            )
            .id(session.id)
        } else {
            ModeEmptyState(
                eyebrow: "Local · Private · Yours",
                headline: "Every conversation starts here.",
                caption: emptyCaption
            ) {
                VStack(spacing: Design.Space.l) {
                    if let record = Launcher.defaultChatModel(in: shell.library.records) {
                        Button("New chat with \(record.displayName)") {
                            shell.newChat()
                        }
                        .buttonStyle(InkButtonStyle())
                        .keyboardShortcut(.defaultAction)
                    }
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
                }
            }
        }
    }

    private var emptyCaption: String {
        if Launcher.defaultChatModel(in: shell.library.records) != nil {
            return "Local, private, yours."
        }
        return "When a chat-capable model lands on your shelf, it opens here."
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
                        if hits.isEmpty {
                            Text("Nothing found.")
                                .font(Design.caption)
                                .foregroundStyle(Design.inkFaint)
                                .padding(Design.Space.m)
                        } else {
                            ForEach(hits, id: \.turnID) { hit in
                                Button {
                                    shell.selectChat(hit.sessionID)
                                } label: {
                                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                                        Text(hit.sessionTitle)
                                            .font(Design.body)
                                            .foregroundStyle(Design.ink)
                                            .lineLimit(1)
                                        SearchSnippet(snippet: hit.snippet)
                                            .font(Design.label)
                                            .foregroundStyle(Design.inkFaint)
                                            .lineLimit(2)
                                    }
                                    .padding(.horizontal, Design.Space.chipX)
                                    .padding(.vertical, Design.Space.s)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .background(
                                        RoundedRectangle(cornerRadius: Design.Radius.control)
                                            .fill(
                                                hoveredHit == hit.turnID
                                                    ? Design.ink.opacity(0.04) : .clear)
                                            .animation(
                                                Design.wash,
                                                value: hoveredHit == hit.turnID))
                                    .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
                                }
                                .buttonStyle(.plain)
                                .onHover { inside in
                                    if inside {
                                        hoveredHit = hit.turnID
                                    } else if hoveredHit == hit.turnID {
                                        hoveredHit = nil
                                    }
                                }
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
        .task(id: query) {
            guard !query.isEmpty else {
                hits = []
                return
            }
            try? await Task.sleep(for: .milliseconds(150))
            guard !Task.isCancelled else { return }
            hits = (try? await shell.kernel.chats.searchChats(query: query)) ?? []
        }
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
        } message: {
            Text("The conversation leaves your history.")
        }
    }

    @ViewBuilder
    private func rowActions(_ session: ChatSession) -> some View {
        Button("Rename…") {
            renameTitle = session.title
            renaming = session
        }
        Button(session.pinned ? "Unpin" : "Pin") {
            setPinned(session, !session.pinned)
        }
        Button(session.archived ? "Unarchive" : "Archive") {
            setArchived(session, !session.archived)
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
        mutate { try await $0.deleteSession(id: session.id) }
        if shell.chatSelection == session.id {
            shell.selectChat(nil)
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
                in: RoundedRectangle(cornerRadius: Design.Radius.control))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
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

struct ImagesPane: View {
    @Bindable var shell: ShellModel
    @State private var showGallery = false

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Images") {
                if !shell.images.arranged.isEmpty {
                    QuietIconButton(glyph: "square.grid.2x2") {
                        showGallery = true
                    }
                    .help("All generations")
                    .accessibilityLabel("Gallery")
                }
            }
            ImagesSurface(shell: shell)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .task { await shell.images.load() }
        .modalScrim(isPresented: showGallery, onDismiss: { showGallery = false }) {
            GallerySheet(shell: shell) {
                showGallery = false
            }
        }
    }

}

struct GallerySheet: View {
    @Bindable var shell: ShellModel
    let onClose: () -> Void
    @State private var deleting: Artifact?
    @State private var hoveredCell: String?

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center, spacing: Design.Space.l) {
                Image(systemName: "photo.stack")
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkSoft)
                    .frame(width: 40, height: 40)
                    .background(
                        Design.cardFill,
                        in: RoundedRectangle(cornerRadius: Design.Radius.control))
                VStack(alignment: .leading, spacing: Design.Space.xxs) {
                    Text("Gallery")
                        .font(Design.title)
                        .tracking(Design.tightTracking)
                    Text(countLine)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
                Spacer()
                SheetCloseButton(action: onClose)
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.gutter)
            .padding(.bottom, Design.Space.xl)
            ScrollView {
                LazyVGrid(
                    columns: [
                        GridItem(.adaptive(minimum: 140), spacing: Design.Space.l)
                    ],
                    spacing: Design.Space.l
                ) {
                    ForEach(shell.images.arranged) { artifact in
                        cell(artifact)
                    }
                }
                .padding(Design.Space.gutter)
            }
        }
        .frame(width: Design.Sheet.gallery.width, height: Design.Sheet.gallery.height)
        .confirmationDialog(
            "Move this image to the Trash?",
            isPresented: Binding(
                get: { deleting != nil },
                set: { if !$0 { deleting = nil } })
        ) {
            Button("Move to Trash", role: .destructive) {
                if let artifact = deleting {
                    let shell = shell
                    Task {
                        await shell.images.delete(artifact)
                        if shell.imagesSelection == artifact.id {
                            shell.selectImages(nil)
                        }
                    }
                }
                deleting = nil
            }
        } message: {
            Text("The file moves to the Trash, not deleted outright.")
        }
    }

    private var countLine: String {
        let count = shell.images.arranged.count
        return count == 1 ? "1 image" : "\(count) images"
    }

    private func cell(_ artifact: Artifact) -> some View {
        Button {
            shell.selectImages(artifact.id)
            onClose()
        } label: {
            VStack(alignment: .leading, spacing: Design.Space.xs) {
                Group {
                    if let image = shell.images.thumbnail(artifact) {
                        Image(nsImage: image)
                            .resizable()
                            .scaledToFill()
                    } else {
                        SkeletonPulse()
                    }
                }
                .frame(width: 140, height: 140)
                .clipShape(RoundedRectangle(cornerRadius: Design.Radius.card))
                Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
                    .lineLimit(1)
                    .opacity(hoveredCell == artifact.id ? 1 : 0)
                    .frame(maxWidth: 140, alignment: .leading)
            }
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside {
                hoveredCell = artifact.id
            } else if hoveredCell == artifact.id {
                hoveredCell = nil
            }
        }
        .animation(Design.wash, value: hoveredCell)
        .task(id: artifact.id) {
            await shell.images.loadThumbnail(artifact)
        }
        .contextMenu {
            Button("Download…") {
                shell.images.download(artifact)
            }
            Divider()
            Button("Delete…", role: .destructive) {
                deleting = artifact
            }
        }
        .help("Show in the conversation")
    }
}

struct VoicePane: View {
    @Bindable var shell: ShellModel

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Voice")
            VoiceSurface(shell: shell)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
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
                Text(eyebrow.uppercased())
                    .font(Design.micro)
                    .tracking(Design.microTracking)
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
