import AppKit
import HedosKernel
import SwiftUI

struct ChatStartHero: View {
    @Bindable var shell: ShellModel
    @State private var draft = ""
    @State private var selectedModelID: String?
    @State private var denyCount = 0
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
            .staggeredArrival(0)
            if !startableGroups.isEmpty {
                modelPicker
                    .staggeredArrival(1)
            }
            startChatField
                .staggeredArrival(2)
            Text(caption)
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .multilineTextAlignment(.center)
                .staggeredArrival(3)
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
            .staggeredArrival(4)
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
            CircleControl(
                glyph: "arrow.up",
                prominent: !sendDisabled,
                label: "Start chat",
                action: launchChat
            )
            .disabled(activeModel == nil)
        }
        .padding(.leading, Design.Space.xl)
        .padding(.trailing, Design.Space.s)
        .padding(.vertical, Design.Space.s)
        .surfaceCard(radius: Design.Radius.bubble)
        .denyShake(on: denyCount, in: RoundedRectangle.soft(Design.Radius.bubble))
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
        guard let record = activeModel else { return }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else {
            denyCount += 1
            fieldFocused = true
            return
        }
        shell.startChat(bound: record, intent: activeIntent, seed: text)
        draft = ""
    }
}

struct ChatSessionsColumn: View {
    @Bindable var shell: ShellModel
    @Binding var query: String
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
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
                    Design.motion(reduceMotion: reduceMotion),
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
            .animation(Design.wash, value: selected)
            .animation(Design.wash, value: hovering)
        }
        .buttonStyle(PressDipStyle())
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
