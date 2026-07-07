import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ShellModel {
    let library: LibraryViewModel
    let images: ImagesViewModel
    let voice: VoiceSurfaceModel

    var mode: AppMode = .library
    var chatSelection: String?
    var imagesSelection: String?
    var voiceSelection: String?
    var librarySelection: String?
    var sessions: [ChatSession] = []
    var sidebarCollapsed = false
    var isFullscreen = false

    var kernel: Kernel { library.kernel }

    init() {
        let library = LibraryViewModel(kernel: Kernel())
        self.library = library
        self.images = ImagesViewModel(kernel: library.kernel)
        self.voice = VoiceSurfaceModel(kernel: library.kernel)
    }

    init(library: LibraryViewModel) {
        self.library = library
        self.images = ImagesViewModel(kernel: library.kernel)
        self.voice = VoiceSurfaceModel(kernel: library.kernel)
    }

    func start() async {
        if let restored = try? await kernel.shellState() {
            mode = restored.mode
            chatSelection = restored.chatSessionID
            imagesSelection = restored.imagesSelection
            voiceSelection = restored.voiceModelID
            librarySelection = restored.libraryModelID
            sidebarCollapsed = restored.sidebarCollapsed
        }
        await refreshSessions()
        await library.rescan()
    }

    func setSidebarCollapsed(_ collapsed: Bool) {
        guard sidebarCollapsed != collapsed else { return }
        sidebarCollapsed = collapsed
        persist()
    }

    func setMode(_ newMode: AppMode) {
        guard mode != newMode else { return }
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

    private func startChat(bound record: ModelRecord) {
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

    @State private var hoveredMode: AppMode?

    var body: some View {
        NavigationSplitView(
            columnVisibility: Binding(
                get: { shell.sidebarCollapsed ? .detailOnly : .all },
                set: { shell.setSidebarCollapsed($0 == .detailOnly) })
        ) {
            sidebar
                .padding(.top, shell.isFullscreen ? Design.Space.l : 0)
                .navigationSplitViewColumnWidth(min: 200, ideal: 220, max: 280)
        } detail: {
            pane
                .background {
                    Design.paper.ignoresSafeArea()
                }
        }
        .frame(minWidth: 860, minHeight: 520)
        .containerBackground(Design.paper, for: .window)
        .toolbarBackground(
            shell.isFullscreen ? AnyShapeStyle(Design.paper) : AnyShapeStyle(.clear),
            for: .windowToolbar
        )
        .toolbarBackgroundVisibility(
            shell.isFullscreen ? .visible : .automatic, for: .windowToolbar)
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

    private var sidebar: some View {
        List {
            ForEach([AppMode.chat, .images, .voice, .library], id: \.self) { mode in
                RailRow(shell: shell, mode: mode, hovered: $hoveredMode)
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                    .listRowInsets(
                        EdgeInsets(
                            top: Design.Space.xxs, leading: -Design.Space.chipX,
                            bottom: Design.Space.xxs, trailing: Design.Space.m))
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .top, spacing: 0) {
            VStack(spacing: 0) {
                HStack(spacing: Design.Space.chipX) {
                    HeptagonMark(size: 22, color: Design.ink)
                    Text("Hedos")
                        .font(.system(size: 17, weight: .semibold))
                        .tracking(Design.tightTracking)
                        .foregroundStyle(Design.ink)
                    Spacer()
                }
                .padding(.horizontal, Design.Space.xl)
                .padding(.top, Design.Space.s)
                .padding(.bottom, Design.Space.xl)
                .accessibilityLabel("Hedos")
                Rectangle()
                    .fill(Design.line)
                    .frame(height: Design.hairlineWidth)
                    .padding(.horizontal, Design.Space.l)
                Color.clear.frame(height: Design.Space.m)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            RailRow(shell: shell, mode: .settings, hovered: $hoveredMode)
                .padding(.leading, Design.Space.s)
                .padding(.trailing, Design.Space.m)
                .padding(.vertical, Design.Space.l)
        }
        .accessibilityIdentifier("shell-rail")
    }

    @ViewBuilder
    private var pane: some View {
        switch shell.mode {
        case .chat:
            ChatPane(shell: shell)
                .transition(.opacity)
        case .images:
            ImagesPane(shell: shell)
                .transition(.opacity)
        case .voice:
            VoicePane(shell: shell)
                .transition(.opacity)
        case .library:
            ModelsPane(shell: shell)
                .transition(.opacity)
        case .settings:
            SettingsPane()
                .transition(.opacity)
        }
    }
}

struct RailRow: View {
    let shell: ShellModel
    let mode: AppMode
    @Binding var hovered: AppMode?

    private var selected: Bool { shell.mode == mode }
    private var hovering: Bool { hovered == mode }

    var body: some View {
        Button {
            shell.setMode(mode)
        } label: {
            HStack(spacing: Design.Space.chipX) {
                Image(systemName: Design.modeGlyph(mode))
                    .symbolVariant(selected ? .fill : .none)
                    .font(Design.glyphNav)
                    .foregroundStyle(selected ? Design.ink : Design.inkSoft)
                    .frame(width: 24)
                Text(Design.modeTitle(mode))
                    .font(Design.body.weight(.medium))
                    .foregroundStyle(selected ? Design.ink : Design.inkSoft)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s + 1)
            .background(
                selected
                    ? Design.ink.opacity(0.08)
                    : hovering ? Design.ink.opacity(0.04) : .clear,
                in: RoundedRectangle(cornerRadius: Design.Radius.inner))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.inner))
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside {
                hovered = mode
            } else if hovered == mode {
                hovered = nil
            }
        }
        .help("\(Design.modeTitle(mode)) — ⌘\(mode.ordinal)")
        .accessibilityLabel(Design.modeTitle(mode))
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityIdentifier("rail-\(mode.rawValue)")
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
                result = result + Text(current)
                current = ""
                marked = true
            case "]" where marked:
                result = result + Text(current).fontWeight(.semibold).foregroundStyle(Design.inkSoft)
                current = ""
                marked = false
            default:
                current.append(character)
            }
        }
        if !current.isEmpty {
            result =
                result
                + (marked
                    ? Text(current).fontWeight(.semibold).foregroundStyle(Design.inkSoft)
                    : Text(current))
        }
        return result
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
    @State private var query = ""

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ChatSessionsColumn(shell: shell, query: $query)
                    .frame(width: Design.Rail.columnWidth)
                ColumnDivider()
                detail
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .navigationTitle("Chat")
        .searchable(text: $query, placement: .toolbar, prompt: "Search chats")
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Menu {
                    Picker(
                        "Show",
                        selection: Binding(
                            get: { shell.sessionFilter },
                            set: { shell.setSessionFilter($0) })
                    ) {
                        Text("Active").tag(ChatSessionFilter.active)
                        Text("Archived").tag(ChatSessionFilter.archived)
                    }
                    .pickerStyle(.inline)
                } label: {
                    Image(systemName: "archivebox")
                }
                .menuIndicator(.hidden)
                .accessibilityLabel("Filter conversations")
                .help("Switch between active and archived chats")
                Button {
                    shell.newChat()
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .disabled(Launcher.defaultChatModel(in: shell.library.records) == nil)
                .help(newChatHelp)
                .accessibilityLabel("New chat")
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
                        Button("New chat with \(record.name)") {
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

    private var newChatHelp: String {
        if let record = Launcher.defaultChatModel(in: shell.library.records) {
            return "Start a new chat with \(record.name)"
        }
        return "No chat-capable model is ready yet"
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

    var body: some View {
        VStack(spacing: 0) {
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
                                    .contentShape(RoundedRectangle(cornerRadius: Design.Radius.inner))
                                }
                                .buttonStyle(.plain)
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
        Button("Export as Markdown…") {
            export(session, json: false)
        }
        Button("Export as JSON…") {
            export(session, json: true)
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
                        .font(Design.body.weight(selected ? .medium : .regular))
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
            .padding(.vertical, Design.Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected
                    ? Design.ink.opacity(0.08)
                    : hovering ? Design.ink.opacity(0.04) : .clear,
                in: RoundedRectangle(cornerRadius: Design.Radius.inner))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.inner))
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
            parts.append(shell.library.record(id: modelID)?.name ?? modelID)
        }
        parts.append(session.updatedAt.formatted(.relative(presentation: .named)))
        return parts.joined(separator: " · ")
    }

}

struct ImagesPane: View {
    @Bindable var shell: ShellModel
    @State private var showGallery = false

    var body: some View {
        ImagesSurface(shell: shell)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle("Images")
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                if !shell.images.arranged.isEmpty {
                    Button {
                        showGallery = true
                    } label: {
                        Image(systemName: "square.grid.2x2")
                    }
                    .help("All generations")
                    .accessibilityLabel("Gallery")
                }
            }
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

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: Design.Space.m) {
                Text("Gallery")
                    .font(Design.title)
                Text(countLine)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
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
            .padding(.horizontal, Design.Space.gutter)
            .padding(.vertical, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
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
        .frame(width: 640, height: 520)
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
            Text("The file moves to the Trash — it is not deleted outright.")
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
                        Rectangle().fill(Design.cardFill)
                    }
                }
                .frame(width: 140, height: 140)
                .clipShape(RoundedRectangle(cornerRadius: Design.Radius.card))
                Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
                    .lineLimit(1)
                    .frame(maxWidth: 140, alignment: .leading)
            }
        }
        .buttonStyle(.plain)
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
        VoiceSurface(shell: shell)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .navigationTitle("Voice")
    }

}

struct SettingsPane: View {
    var body: some View {
        ModeEmptyState(
            eyebrow: "Coming with customization",
            headline: "Nothing to set yet.",
            caption: "Customization arrives here — every model, configurable.")
            .navigationTitle("Settings")
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
                .font(.title2.weight(.semibold))
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
                .multilineTextAlignment(.center)
            Text(caption)
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(2.5)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 380)
            extra()
                .padding(.top, Design.Space.l)
        }
        .padding(Design.Space.pane)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
