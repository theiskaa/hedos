import AppKit
import HedosKernel
import SwiftUI
import UniformTypeIdentifiers

@MainActor
enum CommandCatalog {
    static let navModes: [AppMode] = [.home, .chat, .library, .gateway]

    static func commands(for shell: ShellModel) -> [CommandItem] {
        var items: [CommandItem] = []

        for (index, mode) in navModes.enumerated() {
            items.append(
                CommandItem(
                    id: "nav.\(mode.rawValue)",
                    title: "Go to \(Design.modeTitle(mode))",
                    glyph: navGlyph(mode),
                    keywords: [Design.modeTitle(mode).lowercased(), "open", "go", "switch"],
                    shortcut: KeyHint(modifiers: .command, key: "\(index + 1)"),
                    section: .navigate,
                    perform: { shell.setMode(mode) }))
        }
        items.append(
            CommandItem(
                id: "nav.settings",
                title: "Open Settings",
                glyph: "gearshape",
                keywords: ["settings", "preferences", "options"],
                shortcut: KeyHint(modifiers: .command, key: ","),
                section: .navigate,
                perform: { shell.openSettings() }))

        for section in SettingsSection.allCases {
            items.append(
                CommandItem(
                    id: "settings.\(section.rawValue)",
                    title: "Settings: \(section.title)",
                    glyph: section.glyph,
                    keywords: ["settings", section.title.lowercased()],
                    section: .settings,
                    perform: {
                        shell.openSettings(at: SettingsDestination(section: section, anchor: nil))
                    }))
        }

        items.append(
            CommandItem(
                id: "chat.new", title: "New chat", glyph: "square.and.pencil",
                keywords: ["new", "chat", "compose", "start"],
                shortcut: KeyHint(modifiers: .command, key: "N"),
                section: .chat, perform: { shell.newChat() }))
        items.append(
            CommandItem(
                id: "chat.import", title: "Import chat…", glyph: "square.and.arrow.down",
                keywords: ["import", "open", "json", "restore"],
                section: .chat,
                perform: {
                    let panel = NSOpenPanel()
                    panel.allowedContentTypes = [.json]
                    panel.allowsMultipleSelection = false
                    if panel.runModal() == .OK, let url = panel.url {
                        shell.importChat(from: url)
                    }
                }))
        items.append(
            CommandItem(
                id: "chat.find", title: "Find chats", glyph: "magnifyingglass",
                keywords: ["find", "search", "chats"],
                shortcut: KeyHint(modifiers: .command, key: "F"),
                section: .chat, perform: { shell.setMode(.chat); shell.focusChatSearch() }))
        items.append(
            CommandItem(
                id: "chat.next", title: "Next chat", glyph: "chevron.down",
                keywords: ["next", "forward"],
                shortcut: KeyHint(modifiers: [.command, .shift], key: "]"),
                section: .chat, perform: { shell.selectAdjacentChat(1) }))
        items.append(
            CommandItem(
                id: "chat.prev", title: "Previous chat", glyph: "chevron.up",
                keywords: ["previous", "prev", "back"],
                shortcut: KeyHint(modifiers: [.command, .shift], key: "["),
                section: .chat, perform: { shell.selectAdjacentChat(-1) }))
        items.append(
            CommandItem(
                id: "chat.archived",
                title: shell.sessionFilter == .archived ? "Show active chats" : "Show archived chats",
                glyph: "archivebox",
                keywords: ["archive", "archived", "filter", "active"],
                section: .chat,
                perform: {
                    shell.setSessionFilter(shell.sessionFilter == .archived ? .active : .archived)
                }))
        if !shell.gallery.arranged.isEmpty {
            items.append(
                CommandItem(
                    id: "chat.gallery", title: "Open image gallery", glyph: "square.grid.2x2",
                    keywords: ["gallery", "images", "pictures"],
                    section: .chat, perform: { shell.showingGallery = true }))
        }

        items.append(
            CommandItem(
                id: "view.sidebar",
                title: shell.sidebarCollapsed ? "Expand sidebar" : "Collapse sidebar",
                glyph: "sidebar.left",
                keywords: ["sidebar", "collapse", "expand", "hide"],
                shortcut: KeyHint(modifiers: [.command, .option], key: "S"),
                section: .view,
                perform: { shell.setSidebarCollapsed(!shell.sidebarCollapsed) }))
        items.append(
            CommandItem(
                id: "view.fullscreen", title: "Toggle full screen",
                glyph: "arrow.up.left.and.arrow.down.right",
                keywords: ["fullscreen", "full screen", "maximize"],
                shortcut: KeyHint(modifiers: [.command, .control], key: "F"),
                section: .view,
                perform: { (NSApp.keyWindow ?? NSApp.mainWindow)?.toggleFullScreen(nil) }))

        items.append(
            CommandItem(
                id: "models.rescan", title: "Rescan for models", glyph: "arrow.clockwise",
                keywords: ["rescan", "scan", "refresh", "discover", "models"],
                section: .models, perform: { Task { await shell.library.rescan() } }))
        let gatewayRunning = shell.settings.gatewayStatus.running
        items.append(
            CommandItem(
                id: "gateway.toggle",
                title: gatewayRunning ? "Stop gateway" : "Start gateway",
                glyph: "network",
                keywords: ["gateway", "server", "start", "stop", "serve", "api"],
                section: .gateway,
                perform: { shell.settings.setGatewayEnabled(!gatewayRunning) }))

        for session in shell.sessions where !session.archived {
            items.append(
                CommandItem(
                    id: "session.\(session.id)", title: session.title, subtitle: "Chat",
                    glyph: "bubble.left", keywords: [session.title.lowercased()],
                    isEntity: true, section: .chat,
                    perform: { shell.setMode(.chat); shell.selectChat(session.id) }))
        }
        for record in shell.library.records {
            items.append(
                CommandItem(
                    id: "model.\(record.id)", title: record.displayName, subtitle: "Model",
                    glyph: "square.stack.3d.up",
                    keywords: [record.displayName.lowercased(), record.name.lowercased()],
                    isEntity: true, section: .models,
                    perform: { shell.launch(record) }))
        }

        return items
    }

    private static func navGlyph(_ mode: AppMode) -> String {
        switch mode {
        case .home: "house"
        case .chat: "message"
        case .library: "square.stack.3d.up"
        case .gateway: "network"
        default: "circle"
        }
    }
}
