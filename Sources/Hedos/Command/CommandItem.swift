import SwiftUI

enum CommandSection: String, CaseIterable {
    case navigate
    case chat
    case models
    case gateway
    case view
    case settings

    var title: String {
        switch self {
        case .navigate: "Navigate"
        case .chat: "Chat"
        case .models: "Models"
        case .gateway: "Gateway"
        case .view: "View"
        case .settings: "Settings"
        }
    }
}

struct KeyHint {
    let modifiers: EventModifiers
    let key: String

    var display: String {
        var glyphs = ""
        if modifiers.contains(.control) { glyphs += "⌃" }
        if modifiers.contains(.option) { glyphs += "⌥" }
        if modifiers.contains(.shift) { glyphs += "⇧" }
        if modifiers.contains(.command) { glyphs += "⌘" }
        return glyphs + key
    }
}

struct CommandItem: Identifiable {
    let id: String
    let title: String
    var subtitle: String? = nil
    var glyph: String
    var keywords: [String] = []
    var shortcut: KeyHint? = nil
    var isEntity = false
    let section: CommandSection
    let perform: () -> Void
}

struct Keycap: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Design.micro)
            .foregroundStyle(Design.inkSoft)
            .padding(.horizontal, Design.Space.xs + 1)
            .padding(.vertical, 1)
            .background(Design.inkWash, in: RoundedRectangle.soft(Design.Radius.control))
    }
}

struct KeyHintLabel: View {
    let hint: KeyHint

    var body: some View {
        Keycap(text: hint.display)
    }
}
