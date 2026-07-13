import AppKit
import SwiftUI

@Observable
@MainActor
final class CommandPaletteModel {
    var query = "" {
        didSet { recompute() }
    }
    var selectedIndex = 0
    private(set) var items: [CommandItem] = []
    private(set) var results: [CommandItem] = []

    private static let resultLimit = 50

    func reload(for shell: ShellModel) {
        items = CommandCatalog.commands(for: shell)
        query = ""
        recompute()
    }

    var browsing: Bool {
        query.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private func recompute() {
        selectedIndex = 0
        let trimmed = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !trimmed.isEmpty else {
            results = CommandSection.allCases.flatMap { section in
                items.filter { !$0.isEntity && $0.section == section }
            }
            return
        }
        results =
            items
            .map { ($0, Self.score($0, query: trimmed)) }
            .filter { $0.1 > 0 }
            .sorted { $0.1 != $1.1 ? $0.1 > $1.1 : $0.0.title < $1.0.title }
            .prefix(Self.resultLimit)
            .map { $0.0 }
    }

    func moveSelection(_ delta: Int) {
        let count = results.count
        guard count > 0 else { return }
        selectedIndex = (selectedIndex + delta + count) % count
    }

    func selected() -> CommandItem? {
        let list = results
        guard list.indices.contains(selectedIndex) else { return nil }
        return list[selectedIndex]
    }

    static func score(_ item: CommandItem, query: String) -> Int {
        let title = item.title.lowercased()
        if title == query { return 100 }
        if title.hasPrefix(query) { return 80 }
        if title.contains(query) { return 60 }
        if item.keywords.contains(where: { $0.hasPrefix(query) }) { return 45 }
        if item.keywords.contains(where: { $0.contains(query) }) { return 30 }
        if isSubsequence(query, of: title) { return 12 }
        return 0
    }

    private static func isSubsequence(_ needle: String, of haystack: String) -> Bool {
        var iterator = haystack.makeIterator()
        for character in needle {
            var matched = false
            while let next = iterator.next() {
                if next == character {
                    matched = true
                    break
                }
            }
            if !matched { return false }
        }
        return true
    }
}

struct CommandPaletteView: View {
    @Bindable var model: CommandPaletteModel
    let onClose: () -> Void
    let onPerform: (CommandItem) -> Void

    @Namespace private var selection
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let rowHeight: CGFloat = 42
    private static let headerHeight: CGFloat = 28
    private static let maxListHeight: CGFloat = 380

    var body: some View {
        VStack(spacing: 0) {
            searchBar
            divider
            content
            divider
            footer
        }
        .frame(width: 600)
        .background(Design.paper, in: RoundedRectangle.soft(Design.Radius.bubble))
        .clipShape(RoundedRectangle.soft(Design.Radius.bubble))
        .overlay(
            RoundedRectangle.soft(Design.Radius.bubble)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .shade(Design.Elevation.sheet)
    }

    private var divider: some View {
        Rectangle().fill(Design.line).frame(height: Design.hairlineWidth)
    }

    private var searchBar: some View {
        HStack(spacing: Design.Space.m) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 15, weight: .medium))
                .foregroundStyle(Design.inkFaint)
            PaletteSearchField(
                text: $model.query,
                onMove: { model.moveSelection($0) },
                onSubmit: performSelected,
                onCancel: onClose)
        }
        .padding(.horizontal, Design.Space.xl)
        .frame(height: 54)
    }

    @ViewBuilder
    private var content: some View {
        let list = model.results
        if list.isEmpty {
            Text("No matching commands")
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .frame(maxWidth: .infinity)
                .frame(height: Self.rowHeight * 2)
        } else {
            let raw = rawContentHeight(list)
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        ForEach(Array(list.enumerated()), id: \.element.id) { index, item in
                            if model.browsing,
                                index == 0 || list[index - 1].section != item.section
                            {
                                sectionHeader(item.section)
                            }
                            CommandRow(
                                item: item,
                                selected: index == model.selectedIndex,
                                namespace: selection,
                                height: Self.rowHeight,
                                onTap: { onPerform(item) }
                            )
                            .id(item.id)
                        }
                    }
                    .padding(.horizontal, Design.Space.m)
                    .padding(.vertical, Design.Space.xs)
                    .animation(
                        reduceMotion ? nil : Design.snap, value: model.selectedIndex)
                }
                .scrollDisabled(raw <= Self.maxListHeight)
                .frame(height: min(raw, Self.maxListHeight))
                .onChange(of: model.selectedIndex) { _, index in
                    guard model.results.indices.contains(index) else { return }
                    withAnimation(reduceMotion ? nil : Design.snap) {
                        proxy.scrollTo(model.results[index].id, anchor: .center)
                    }
                }
                .onChange(of: model.query) { _, _ in
                    if let first = model.results.first {
                        proxy.scrollTo(first.id, anchor: .top)
                    }
                }
            }
        }
    }

    private func sectionHeader(_ section: CommandSection) -> some View {
        Text(section.title)
            .font(Design.label.weight(.medium))
            .foregroundStyle(Design.inkFaint)
            .padding(.horizontal, Design.Space.m)
            .frame(height: Self.headerHeight, alignment: .bottomLeading)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func rawContentHeight(_ list: [CommandItem]) -> CGFloat {
        let headerCount = model.browsing ? Set(list.map(\.section)).count : 0
        let rows = CGFloat(list.count) * Self.rowHeight
        let headers = CGFloat(headerCount) * Self.headerHeight
        return rows + headers + Design.Space.xs * 2
    }

    private var footer: some View {
        HStack(spacing: Design.Space.l) {
            hint("↑↓", "navigate")
            hint("↵", "open")
            hint("esc", "close")
            Spacer()
        }
        .padding(.horizontal, Design.Space.xl)
        .frame(height: 34)
    }

    private func hint(_ key: String, _ label: String) -> some View {
        HStack(spacing: Design.Space.xs) {
            Keycap(text: key)
            Text(label)
                .font(Design.micro)
                .foregroundStyle(Design.inkFaint)
        }
    }

    private func performSelected() {
        guard let item = model.selected() else { return }
        onPerform(item)
    }
}

private struct CommandRow: View {
    let item: CommandItem
    let selected: Bool
    let namespace: Namespace.ID
    let height: CGFloat
    let onTap: () -> Void

    @State private var hovering = false

    var body: some View {
        HStack(spacing: Design.Space.m) {
            Image(systemName: item.glyph)
                .font(Design.glyphInline)
                .foregroundStyle(selected ? Design.ink : Design.inkSoft)
                .frame(width: 20)
            Text(item.title)
                .font(Design.body.weight(selected ? .medium : .regular))
                .foregroundStyle(selected ? Design.ink : Design.inkSoft)
                .lineLimit(1)
                .truncationMode(.tail)
            Spacer(minLength: Design.Space.m)
            trailing
        }
        .padding(.horizontal, Design.Space.m)
        .frame(height: height)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background {
            if selected {
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(Design.inkWash)
                    .padding(.vertical, 3)
                    .matchedGeometryEffect(id: "paletteSelection", in: namespace)
            } else if hovering {
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(Design.inkWash.opacity(0.5))
                    .padding(.vertical, 3)
            }
        }
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture { onTap() }
    }

    @ViewBuilder
    private var trailing: some View {
        if let shortcut = item.shortcut {
            KeyHintLabel(hint: shortcut)
        } else if let subtitle = item.subtitle {
            Text(subtitle)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
        }
    }
}

private struct PaletteSearchField: NSViewRepresentable {
    @Binding var text: String
    var onMove: (Int) -> Void
    var onSubmit: () -> Void
    var onCancel: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 15)
        field.textColor = NSColor(Design.ink)
        field.maximumNumberOfLines = 1
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        field.lineBreakMode = .byTruncatingTail
        field.placeholderAttributedString = NSAttributedString(
            string: "Search actions or go to…",
            attributes: [
                .foregroundColor: NSColor(Design.inkFaint),
                .font: NSFont.systemFont(ofSize: 15),
            ])
        field.delegate = context.coordinator
        field.stringValue = text
        DispatchQueue.main.async { field.window?.makeFirstResponder(field) }
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        context.coordinator.parent = self
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: PaletteSearchField

        init(_ parent: PaletteSearchField) {
            self.parent = parent
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else { return }
            parent.text = field.stringValue
        }

        func control(
            _ control: NSControl, textView: NSTextView, doCommandBy selector: Selector
        ) -> Bool {
            switch selector {
            case #selector(NSResponder.moveUp(_:)):
                parent.onMove(-1)
                return true
            case #selector(NSResponder.moveDown(_:)):
                parent.onMove(1)
                return true
            case #selector(NSResponder.insertNewline(_:)),
                #selector(NSResponder.insertLineBreak(_:)):
                parent.onSubmit()
                return true
            case #selector(NSResponder.cancelOperation(_:)):
                parent.onCancel()
                return true
            default:
                return false
            }
        }
    }
}
