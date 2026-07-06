import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class ShellModel {
    let library: LibraryViewModel

    var mode: AppMode = .library
    var chatSelection: String?
    var imagesSelection: String?
    var voiceSelection: String?
    var librarySelection: String?
    var sessions: [ChatSession] = []
    var pendingConfirm: String?
    var canvasPrefill: CanvasPrefill?
    private var confirmJumps = false

    var kernel: Kernel { library.kernel }

    init() {
        self.library = LibraryViewModel(kernel: Kernel())
    }

    init(library: LibraryViewModel) {
        self.library = library
    }

    func start() async {
        if let restored = try? await kernel.shellState() {
            mode = restored.mode
            chatSelection = restored.chatSessionID
            imagesSelection = restored.imagesSelection
            voiceSelection = restored.voiceModelID
            librarySelection = restored.libraryModelID
        }
        await refreshSessions()
        await library.rescan()
    }

    func setMode(_ newMode: AppMode) {
        guard mode != newMode else { return }
        mode = newMode
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
        if let prefill = canvasPrefill, imagesModelID(id) != prefill.artifact.modelID {
            canvasPrefill = nil
        }
        persist()
    }

    func selectVoice(_ id: String?) {
        voiceSelection = id
        persist()
    }

    func selectLibrary(_ id: String?) {
        librarySelection = id
        guard let record = library.record(id: id) else {
            persist()
            return
        }
        if needsConfirmation(record) {
            confirmJumps = true
            pendingConfirm = record.id
        } else {
            launch(record)
        }
        persist()
    }

    func resolutionFinished(confirmed: Bool) {
        let recordID = pendingConfirm
        let jumps = confirmJumps
        pendingConfirm = nil
        confirmJumps = false
        guard confirmed, jumps, let record = library.record(id: recordID) else { return }
        launch(record)
        persist()
    }

    func newChat() {
        guard let record = Launcher.defaultChatModel(in: library.records) else { return }
        startChat(bound: record)
    }

    func openParams(_ artifact: Artifact) {
        canvasPrefill = CanvasPrefill(artifact: artifact)
        imagesSelection = "model:\(artifact.modelID)"
        persist()
    }

    func prefill(for record: ModelRecord) -> CanvasPrefill? {
        guard let canvasPrefill, canvasPrefill.artifact.modelID == record.id else { return nil }
        return canvasPrefill
    }

    func refreshSessions() async {
        sessions = (try? await kernel.chats.sessions()) ?? sessions
    }

    func session(id: String?) -> ChatSession? {
        guard let id else { return nil }
        return sessions.first { $0.id == id }
    }

    func imagesModelID(_ selection: String?) -> String? {
        guard let selection, selection.hasPrefix("model:") else { return nil }
        return String(selection.dropFirst("model:".count))
    }

    private func launch(_ record: ModelRecord) {
        switch Launcher.destination(for: record) {
        case .chat:
            startChat(bound: record)
        case .images:
            canvasPrefill = nil
            imagesSelection = "model:\(record.id)"
            librarySelection = nil
            mode = .images
            persist()
        case .voice:
            voiceSelection = record.id
            librarySelection = nil
            mode = .voice
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

    private func needsConfirmation(_ record: ModelRecord) -> Bool {
        record.runtime.resolved == .auto && record.runtime.confirmedAt == nil
            && record.runtime.tier != .recipeNeeded
    }

    private func persist() {
        let state = ShellState(
            mode: mode,
            chatSessionID: chatSelection,
            imagesSelection: imagesSelection,
            voiceModelID: voiceSelection,
            libraryModelID: librarySelection)
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
            ModeRail(shell: shell)
            Divider()
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 210, ideal: 240)
            } detail: {
                detail
            }
        }
        .frame(minWidth: 820, minHeight: 480)
        .tint(Design.accent)
        .task { await shell.start() }
        .sheet(
            isPresented: Binding(
                get: { shell.pendingConfirm != nil },
                set: { if !$0 { shell.resolutionFinished(confirmed: false) } })
        ) {
            if let record = shell.library.record(id: shell.pendingConfirm) {
                ResolutionSheet(record: record, library: shell.library) { confirmed in
                    shell.resolutionFinished(confirmed: confirmed)
                }
            }
        }
    }

    @ViewBuilder
    private var sidebar: some View {
        switch shell.mode {
        case .chat:
            ChatSidebar(shell: shell)
        case .images:
            ImagesSidebar(shell: shell)
        case .voice:
            VoiceSidebar(shell: shell)
        case .library:
            LibrarySidebar(
                model: shell.library,
                selection: Binding(
                    get: { shell.librarySelection },
                    set: { shell.selectLibrary($0) }))
        case .settings:
            SettingsSidebar()
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch shell.mode {
        case .chat:
            ChatDetail(shell: shell)
        case .images:
            ImagesDetail(shell: shell)
        case .voice:
            VoiceDetail(shell: shell)
        case .library:
            LibraryDetail(model: shell.library, selectedID: shell.librarySelection)
        case .settings:
            SettingsPane()
        }
    }
}

struct ModeRail: View {
    let shell: ShellModel

    var body: some View {
        VStack(spacing: 6) {
            ForEach([AppMode.chat, .images, .voice, .library], id: \.self) { mode in
                railButton(mode)
            }
            Spacer()
            railButton(.settings)
        }
        .padding(.vertical, 10)
        .frame(width: 56)
        .frame(maxHeight: .infinity)
        .background(.bar)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Modes")
    }

    private func railButton(_ mode: AppMode) -> some View {
        let selected = shell.mode == mode
        return Button {
            shell.setMode(mode)
        } label: {
            Image(systemName: Design.modeGlyph(mode))
                .symbolVariant(selected ? .fill : .none)
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(selected ? AnyShapeStyle(.primary) : AnyShapeStyle(.tertiary))
                .frame(width: 38, height: 32)
                .background(
                    selected ? AnyShapeStyle(.quaternary.opacity(0.6)) : AnyShapeStyle(.clear),
                    in: RoundedRectangle(cornerRadius: 8))
                .contentShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .help("\(Design.modeTitle(mode)) — ⌘\(mode.ordinal)")
        .accessibilityLabel(Design.modeTitle(mode))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct SidebarSectionHeader: View {
    let title: String

    var body: some View {
        Text(title.uppercased())
            .font(.system(size: 10, weight: .medium))
            .tracking(0.8)
            .foregroundStyle(.tertiary)
    }
}

struct ChatSidebar: View {
    @Bindable var shell: ShellModel

    var body: some View {
        List(
            selection: Binding(
                get: { shell.chatSelection },
                set: { shell.selectChat($0) })
        ) {
            if shell.sessions.isEmpty {
                Text("No conversations yet.")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
            } else {
                ForEach(shell.sessions) { session in
                    ChatSessionRow(session: session, shell: shell)
                        .tag(session.id)
                }
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("Chat")
        .task { await shell.refreshSessions() }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    shell.newChat()
                } label: {
                    Label("New Chat", systemImage: "square.and.pencil")
                }
                .help(newChatHelp)
                .disabled(Launcher.defaultChatModel(in: shell.library.records) == nil)
            }
        }
    }

    private var newChatHelp: String {
        if let record = Launcher.defaultChatModel(in: shell.library.records) {
            return "Start a new chat with \(record.name)"
        }
        return "No chat-capable model is ready yet"
    }
}

private struct ChatSessionRow: View {
    let session: ChatSession
    let shell: ShellModel

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(session.title)
                .font(.system(size: 13))
                .lineLimit(1)
            Text(subtitle)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .lineLimit(1)
        }
        .padding(.vertical, 1)
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

struct ChatDetail: View {
    let shell: ShellModel

    var body: some View {
        if let session = shell.session(id: shell.chatSelection) {
            if let record = shell.library.record(id: session.modelID ?? ""),
                Launcher.destination(for: record) == .chat
            {
                ChatView(record: record, kernel: shell.kernel)
                    .id(session.id)
            } else if shell.library.isScanning && shell.library.records.isEmpty {
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ModeEmptyState(
                    glyph: "bubble.left",
                    headline: "This chat's model is gone.",
                    caption: "The model it was bound to is no longer runnable on the shelf.")
            }
        } else {
            chatEmptyState
        }
    }

    private var chatEmptyState: some View {
        ModeEmptyState(
            glyph: "bubble.left",
            headline: "Every conversation starts here.",
            caption: captionText
        ) {
            if let record = Launcher.defaultChatModel(in: shell.library.records) {
                Button("New chat with \(record.name)") {
                    shell.newChat()
                }
                .controlSize(.regular)
            }
        }
    }

    private var captionText: String {
        if Launcher.defaultChatModel(in: shell.library.records) != nil {
            return "Local, private, yours."
        }
        return "When a chat-capable model lands on your shelf, it opens here."
    }
}

struct ImagesSidebar: View {
    @Bindable var shell: ShellModel

    var body: some View {
        List(
            selection: Binding(
                get: { shell.imagesSelection },
                set: { shell.selectImages($0) })
        ) {
            Label("Gallery", systemImage: "photo.stack")
                .font(.system(size: 13))
                .tag("gallery")
            Section {
                if imageModels.isEmpty {
                    Text("No image models yet.")
                        .font(.system(size: 12))
                        .foregroundStyle(.tertiary)
                } else {
                    ForEach(imageModels) { record in
                        ModelRow(record: record)
                            .tag("model:\(record.id)")
                    }
                }
            } header: {
                SidebarSectionHeader(title: "Canvas")
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("Images")
    }

    private var imageModels: [ModelRecord] {
        Launcher.models(in: shell.library.records, for: .images)
    }
}

struct ImagesDetail: View {
    let shell: ShellModel

    var body: some View {
        if shell.imagesSelection == "gallery" {
            GalleryView(kernel: shell.kernel, shelf: shell.library.records) { artifact in
                shell.openParams(artifact)
            }
        } else if let record = shell.library.record(
            id: shell.imagesModelID(shell.imagesSelection)),
            Launcher.destination(for: record) == .images
        {
            ImageCanvasView(
                record: record,
                kernel: shell.kernel,
                prefill: shell.prefill(for: record)?.artifact
            ) {
                shell.selectImages("gallery")
            }
            .id("\(record.id):\(shell.prefill(for: record)?.id.uuidString ?? "")")
        } else if shell.library.isScanning && shell.library.records.isEmpty {
            ProgressView()
                .controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ModeEmptyState(
                glyph: "photo",
                headline: "A sentence in, an image out.",
                caption: "Pick an image model to open its canvas, or browse everything generated in the gallery.")
        }
    }
}

struct VoiceSidebar: View {
    @Bindable var shell: ShellModel

    var body: some View {
        List(
            selection: Binding(
                get: { shell.voiceSelection },
                set: { shell.selectVoice($0) })
        ) {
            if voiceModels.isEmpty {
                Text("No voice models yet.")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
            } else {
                Section {
                    ForEach(voiceModels) { record in
                        ModelRow(record: record)
                            .tag(record.id)
                    }
                } header: {
                    SidebarSectionHeader(title: "Voices")
                }
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("Voice")
    }

    private var voiceModels: [ModelRecord] {
        Launcher.models(in: shell.library.records, for: .voice)
    }
}

struct VoiceDetail: View {
    let shell: ShellModel

    var body: some View {
        if let record = shell.library.record(id: shell.voiceSelection),
            Launcher.destination(for: record) == .voice
        {
            VoiceView(record: record, kernel: shell.kernel)
                .id(record.id)
        } else if shell.library.isScanning && shell.library.records.isEmpty {
            ProgressView()
                .controlSize(.small)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ModeEmptyState(
                glyph: "waveform",
                headline: "Your Mac can speak.",
                caption: "Pick a voice model and give it something to say.")
        }
    }
}

struct SettingsSidebar: View {
    var body: some View {
        List {
            Text("Nothing to configure yet.")
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)
        }
        .listStyle(.sidebar)
        .navigationTitle("Settings")
    }
}

struct SettingsPane: View {
    var body: some View {
        ModeEmptyState(
            glyph: "gearshape",
            headline: "Nothing to set yet.",
            caption: "Customization arrives here — every model, configurable.")
    }
}

struct ModeEmptyState<Extra: View>: View {
    let glyph: String
    let headline: String
    let caption: String
    @ViewBuilder let extra: () -> Extra

    init(
        glyph: String, headline: String, caption: String,
        @ViewBuilder extra: @escaping () -> Extra = { EmptyView() }
    ) {
        self.glyph = glyph
        self.headline = headline
        self.caption = caption
        self.extra = extra
    }

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: glyph)
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(.quaternary)
            Text(headline)
                .font(Design.plaque(18))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Text(caption)
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 340)
            extra()
                .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(28)
    }
}
