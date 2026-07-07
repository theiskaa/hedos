import SwiftUI

struct InkSearchField: View {
    let placeholder: String
    @Binding var query: String
    var fill: Color = Design.paper
    var focusTick: Int = 0
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: Design.Space.s) {
            Image(systemName: "magnifyingglass")
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkFaint)
            TextField(
                "", text: $query,
                prompt: Text(placeholder).foregroundStyle(Design.inkFaint)
            )
            .textFieldStyle(.plain)
            .font(Design.caption)
            .foregroundStyle(Design.ink)
            .focused($focused)
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkFaint)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, Design.Space.chipX)
        .padding(.vertical, Design.Space.s)
        .background(fill, in: Capsule())
        .overlay(
            Capsule()
                .strokeBorder(
                    focused
                        ? AnyShapeStyle(Design.ink.opacity(0.35))
                        : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
        .onExitCommand { query = "" }
        .onChange(of: focusTick) { _, _ in
            focused = true
        }
        .accessibilityLabel(placeholder)
    }
}

struct InkSidebarRow<ID: Hashable>: View {
    let id: ID
    let glyph: String
    let title: String
    var annotation: String? = nil
    let selected: Bool
    var collapsed: Bool = false
    @Binding var hovered: ID?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            if collapsed {
                tile
            } else {
                pill
            }
        }
        .buttonStyle(.plain)
        .onHover { inside in
            if inside {
                hovered = id
            } else if hovered == id {
                hovered = nil
            }
        }
        .accessibilityLabel(title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private var lit: Bool {
        selected || hovered == id
    }

    private var washColor: Color {
        selected
            ? Design.ink.opacity(0.08)
            : hovered == id ? Design.ink.opacity(0.04) : .clear
    }

    private var tile: some View {
        Image(systemName: glyph)
            .symbolVariant(selected ? .fill : .none)
            .font(Design.glyphNav)
            .foregroundStyle(lit ? Design.ink : Design.inkSoft)
            .frame(width: 44, height: 36)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.inner)
                    .fill(washColor))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.inner))
            .animation(Design.wash, value: selected)
            .animation(Design.wash, value: hovered == id)
            .help(title)
    }

    private var pill: some View {
        HStack(spacing: Design.Space.chipX) {
            Image(systemName: glyph)
                .symbolVariant(selected ? .fill : .none)
                .font(Design.glyphPrimary)
                .foregroundStyle(lit ? Design.ink : Design.inkSoft)
                .frame(width: 22)
            Text(title)
                .font(Design.body)
                .fontWeight(.medium)
                .foregroundStyle(lit ? Design.ink : Design.inkSoft)
            Spacer(minLength: 0)
            if let annotation {
                Text(annotation.uppercased())
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.horizontal, Design.Space.l)
        .padding(.vertical, Design.Space.s + 1)
        .background(Capsule().fill(washColor))
        .contentShape(Capsule())
        .animation(Design.wash, value: selected)
        .animation(Design.wash, value: hovered == id)
    }
}

struct SidebarCollapseToggle: View {
    let collapsed: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "sidebar.left")
                .font(Design.glyphPrimary)
                .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                .frame(width: 28, height: 28)
                .background(Circle().fill(Design.ink.opacity(hovering ? 0.06 : 0.04)))
                .contentShape(Circle())
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(collapsed ? "Expand the sidebar" : "Collapse the sidebar")
        .accessibilityLabel(collapsed ? "Expand the sidebar" : "Collapse the sidebar")
    }
}

struct QuietIconButton: View {
    let glyph: String
    var fill: Bool = false
    let action: () -> Void
    @State private var hovering = false
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        Button(action: action) {
            Image(systemName: glyph)
                .symbolVariant(fill ? .fill : .none)
                .font(Design.glyphPrimary)
                .foregroundStyle(hovering && enabled ? Design.ink : Design.inkSoft)
                .frame(width: 28, height: 28)
                .background(
                    Circle().fill(Design.ink.opacity(hovering && enabled ? 0.06 : 0.04)))
                .contentShape(Circle())
                .opacity(enabled ? 1 : 0.4)
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

struct PaneHeader<Actions: View>: View {
    let title: String
    @ViewBuilder let actions: () -> Actions

    var body: some View {
        HStack(spacing: Design.Space.s) {
            Text(title)
                .font(Design.paneTitle)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
            Spacer(minLength: 0)
            actions()
        }
        .padding(.horizontal, Design.Space.gutter)
        .padding(.top, Design.Space.xxl)
        .padding(.bottom, Design.Space.s)
    }
}

extension PaneHeader where Actions == EmptyView {
    init(title: String) {
        self.init(title: title) { EmptyView() }
    }
}

struct CollapsingSidebar<Expanded: View, Collapsed: View>: View {
    let collapsed: Bool
    @ViewBuilder let expanded: () -> Expanded
    @ViewBuilder let collapsedContent: () -> Collapsed

    var body: some View {
        ZStack(alignment: .topLeading) {
            expanded()
                .frame(width: 224, alignment: .leading)
                .opacity(collapsed ? 0 : 1)
                .allowsHitTesting(!collapsed)
                .disabled(collapsed)
                .accessibilityHidden(collapsed)
            collapsedContent()
                .frame(width: 84)
                .opacity(collapsed ? 1 : 0)
                .allowsHitTesting(collapsed)
                .disabled(!collapsed)
                .accessibilityHidden(!collapsed)
        }
        .frame(width: collapsed ? 84 : 224, alignment: .leading)
        .clipped()
        .frame(maxHeight: .infinity, alignment: .top)
        .background(Design.surface.ignoresSafeArea())
    }
}
