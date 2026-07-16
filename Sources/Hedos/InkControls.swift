import AppKit
import HedosKernel
import SwiftUI

struct InlineRenameField: NSViewRepresentable {
    @Binding var text: String
    var pointSize: CGFloat = 13
    var weight: NSFont.Weight = .medium
    var onCommit: () -> Void
    var onCancel: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField()
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: pointSize, weight: weight)
        field.textColor = NSColor(Design.ink)
        field.maximumNumberOfLines = 1
        field.cell?.wraps = false
        field.cell?.isScrollable = true
        field.lineBreakMode = .byTruncatingTail
        field.delegate = context.coordinator
        field.stringValue = text
        DispatchQueue.main.async {
            field.window?.makeFirstResponder(field)
            if let editor = field.currentEditor() as? NSTextView {
                editor.insertionPointColor = NSColor(Design.ink)
                editor.selectedTextAttributes = [
                    .backgroundColor: NSColor(Design.ink).withAlphaComponent(0.16),
                    .foregroundColor: NSColor(Design.ink),
                ]
                editor.selectAll(nil)
            }
        }
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        context.coordinator.parent = self
        if field.stringValue != text {
            field.stringValue = text
        }
    }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var parent: InlineRenameField
        private var cancelled = false

        init(_ parent: InlineRenameField) {
            self.parent = parent
        }

        func controlTextDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextField else { return }
            parent.text = field.stringValue
        }

        func controlTextDidEndEditing(_ notification: Notification) {
            if cancelled {
                cancelled = false
                parent.onCancel()
            } else {
                parent.onCommit()
            }
        }

        func control(
            _ control: NSControl, textView: NSTextView, doCommandBy selector: Selector
        ) -> Bool {
            switch selector {
            case #selector(NSResponder.insertNewline(_:)),
                #selector(NSResponder.insertLineBreak(_:)):
                control.window?.makeFirstResponder(nil)
                return true
            case #selector(NSResponder.cancelOperation(_:)):
                cancelled = true
                control.window?.makeFirstResponder(nil)
                return true
            default:
                return false
            }
        }
    }
}

struct InkSlider: View {
    let range: ClosedRange<Double>
    let value: Double
    let isSet: Bool
    let onChange: (Double) -> Void
    var label: String = "Value"
    @State private var hovering = false
    @State private var grabOffset: CGFloat?
    @FocusState private var focused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var fraction: Double {
        guard range.upperBound > range.lowerBound else { return 0 }
        return ((value - range.lowerBound) / (range.upperBound - range.lowerBound))
            .clamped(to: 0...1)
    }

    var body: some View {
        GeometryReader { proxy in
            let width = proxy.size.width
            let thumbX = fraction * (width - 14)
            ZStack(alignment: .leading) {
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(Design.line)
                    .frame(height: 4)
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(isSet ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.inkFaint))
                    .frame(width: max(7, thumbX + 7), height: 4)
                Circle()
                    .fill(isSet ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface))
                    .overlay(
                        Circle().strokeBorder(
                            isSet ? Design.paper.opacity(0.35) : Design.inkFaint,
                            lineWidth: Design.hairlineWidth))
                    .frame(width: 14, height: 14)
                    .shadow(
                        color: Design.shadowColor.opacity(isSet ? 0.25 : 0.10),
                        radius: 3, x: 0, y: 1)
                    .scaleEffect(hovering && !reduceMotion ? 1.15 : 1)
                    .offset(x: thumbX)
                    .animation(Design.wash, value: hovering)
            }
            .frame(height: 18)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { gesture in
                        let usable = max(1, width - 14)
                        let offset: CGFloat
                        if let grabOffset {
                            offset = grabOffset
                        } else {
                            let delta = gesture.location.x - (thumbX + 7)
                            offset = abs(delta) <= 9 ? delta : 0
                            grabOffset = offset
                        }
                        let raw = ((gesture.location.x - offset - 7) / usable)
                            .clamped(to: 0...1)
                        onChange(
                            range.lowerBound + raw * (range.upperBound - range.lowerBound))
                    }
                    .onEnded { _ in grabOffset = nil })
            .onHover { hovering = $0 }
        }
        .frame(height: 18)
        .focusable(interactions: .activate)
        .focused($focused)
        .focusEffectDisabled()
        .overlay {
            if focused {
                RoundedRectangle.soft(Design.Radius.control)
                    .inset(by: -3)
                    .stroke(Design.ink.opacity(0.35), lineWidth: Design.hairlineWidth)
            }
        }
        .onKeyPress(.leftArrow) {
            nudge(-1)
            return .handled
        }
        .onKeyPress(.rightArrow) {
            nudge(1)
            return .handled
        }
        .accessibilityElement()
        .accessibilityLabel(label)
        .accessibilityValue(isSet ? String(format: "%.2f", value) : "auto")
        .accessibilityAdjustableAction { direction in
            let step = (range.upperBound - range.lowerBound) / 100
            switch direction {
            case .increment: onChange((value + step).clamped(to: range))
            case .decrement: onChange((value - step).clamped(to: range))
            @unknown default: break
            }
        }
    }

    private func nudge(_ direction: Double) {
        let step = (range.upperBound - range.lowerBound) / 20
        onChange((value + direction * step).clamped(to: range))
    }
}

struct InkToggle: View {
    let isOn: Bool
    let isSet: Bool
    let onToggle: (Bool) -> Void
    var label: String = "Toggle"
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button {
            onToggle(!isOn)
        } label: {
            ZStack(alignment: isOn ? .trailing : .leading) {
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(
                        isOn && isSet
                            ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface))
                RoundedRectangle.soft(Design.Radius.control)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth)
                Circle()
                    .fill(
                        isOn && isSet
                            ? AnyShapeStyle(Design.paper)
                            : isSet
                                ? AnyShapeStyle(Design.inkSoft)
                                : AnyShapeStyle(Design.inkFaint))
                    .frame(width: 14, height: 14)
                    .padding(3)
            }
            .frame(width: 34, height: 20)
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
            .animation(
                Design.snapMotion(reduceMotion: reduceMotion), value: isOn)
        }
        .buttonStyle(TogglePressStyle(reduceMotion: reduceMotion))
        .inkFocusRing(RoundedRectangle.soft(Design.Radius.control))
        .accessibilityLabel(label)
        .accessibilityValue(isSet ? (isOn ? "on" : "off") : "auto")
        .accessibilityAddTraits(.isToggle)
    }
}

private struct TogglePressStyle: ButtonStyle {
    let reduceMotion: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed && !reduceMotion ? 0.96 : 1)
            .animation(Design.press, value: configuration.isPressed)
    }
}

private struct FlatButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
    }
}

struct InkSegmented<Value: Hashable>: View {
    let segments: [(value: Value, label: String, icon: String?)]
    let selection: Value?
    let onSelect: (Value) -> Void
    @Namespace private var thumb
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Array(segments.enumerated()), id: \.offset) { _, segment in
                let isOn = selection == segment.value
                Button {
                    onSelect(segment.value)
                } label: {
                    HStack(spacing: Design.Space.xxs) {
                        if let icon = segment.icon {
                            Image(systemName: icon)
                                .font(.system(size: 11, weight: .medium))
                        }
                        Text(segment.label)
                            .font(Design.label.weight(isOn ? .semibold : .regular))
                            .lineLimit(1)
                            .fixedSize()
                    }
                    .foregroundStyle(isOn ? Design.ink : Design.inkSoft)
                    .padding(.horizontal, Design.Space.m)
                    .padding(.vertical, Design.Space.xs + 2)
                    .background {
                        if isOn {
                            Capsule(style: .continuous)
                                .fill(Design.surface)
                                .overlay(
                                    Capsule(style: .continuous)
                                        .strokeBorder(Design.line.opacity(0.7), lineWidth: 0.5))
                                .matchedGeometryEffect(id: "thumb", in: thumb)
                        }
                    }
                    .contentShape(Capsule(style: .continuous))
                }
                .buttonStyle(FlatButtonStyle())
                .accessibilityLabel(segment.label)
                .accessibilityAddTraits(isOn ? .isSelected : [])
            }
        }
        .padding(3)
        .background(Design.inkWash, in: Capsule(style: .continuous))
        .overlay(
            Capsule(style: .continuous)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .animation(Design.snapMotion(reduceMotion: reduceMotion), value: selection)
    }
}

extension InkSegmented where Value == String {
    init(values: [String], selection: String?, onSelect: @escaping (String) -> Void) {
        self.init(
            segments: values.map { (value: $0, label: $0, icon: nil) },
            selection: selection, onSelect: onSelect)
    }
}

enum InkControlSize {
    case compact
    case settings
}

struct InkField: View {
    enum FieldShape {
        case rounded
        case capsule
    }

    let placeholder: String
    @Binding var text: String
    var shape: FieldShape = .rounded
    var size: InkControlSize = .compact
    var glyph: String? = nil
    var font: Font = Design.caption
    var onSubmit: (() -> Void)? = nil
    var onFocusLost: (() -> Void)? = nil
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: Design.Space.s) {
            if size == .settings, let glyph {
                Image(systemName: glyph)
                    .font(Design.glyphInline)
                    .foregroundStyle(Design.inkFaint)
            }
            TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(Design.inkFaint))
                .textFieldStyle(.plain)
                .font(size == .settings ? Design.body : font)
                .foregroundStyle(Design.ink)
                .focused($focused)
                .onSubmit { onSubmit?() }
        }
        .padding(.horizontal, size == .settings ? Design.Space.l : Design.Space.chipX)
        .padding(.vertical, size == .settings ? 0 : Design.Space.xs + 1)
        .frame(height: size == .settings ? Design.Control.fieldHeight : nil)
        .background(size == .settings ? Design.surface2 : Design.surface, in: fieldShape)
        .overlay(
            fieldShape.strokeBorder(
                focused ? AnyShapeStyle(Design.accent.opacity(0.55)) : AnyShapeStyle(Design.line),
                lineWidth: focused && size == .settings
                    ? Design.hairlineWidth * 1.5 : Design.hairlineWidth))
        .contentShape(Rectangle())
        .onTapGesture { focused = true }
        .onChange(of: focused) { _, isFocused in
            if !isFocused { onFocusLost?() }
        }
    }

    private var fieldShape: AnyInsettableShape {
        switch shape {
        case .rounded: AnyInsettableShape(RoundedRectangle.soft(Design.Radius.control))
        case .capsule: AnyInsettableShape(Capsule())
        }
    }
}

struct InkTextArea: View {
    let placeholder: String
    @Binding var text: String
    var lines: ClosedRange<Int> = 2...6
    var resizable = false
    @FocusState private var focused: Bool
    @State private var height: CGFloat = 88
    @State private var dragBase: CGFloat?

    var body: some View {
        Group {
            if resizable {
                ZStack(alignment: .topLeading) {
                    InkTextViewRepresentable(text: $text, focused: $focused)
                        .padding(.horizontal, Design.Space.s)
                        .padding(.vertical, Design.Space.xs)
                    if text.isEmpty {
                        Text(placeholder)
                            .font(Design.caption)
                            .foregroundStyle(Design.inkFaint)
                            .padding(.leading, Design.Space.chipX + 1)
                            .padding(.top, Design.Space.m + 1)
                            .allowsHitTesting(false)
                    }
                }
                .frame(height: height)
                .overlay(alignment: .bottomTrailing) {
                    ResizeGrip()
                        .gesture(
                            DragGesture(minimumDistance: 0)
                                .onChanged { gesture in
                                    let base = dragBase ?? height
                                    dragBase = base
                                    height = (base + gesture.translation.height)
                                        .clamped(to: 56...360)
                                }
                                .onEnded { _ in
                                    dragBase = nil
                                })
                        .accessibilityLabel("Resize")
                }
            } else {
                TextField(
                    "", text: $text,
                    prompt: Text(placeholder).foregroundStyle(Design.inkFaint),
                    axis: .vertical
                )
                .textFieldStyle(.plain)
                .font(Design.caption)
                .foregroundStyle(Design.ink)
                .lineLimit(lines.lowerBound...lines.upperBound)
                .focused($focused)
                .padding(.horizontal, Design.Space.chipX)
                .padding(.vertical, Design.Space.s)
            }
        }
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.control))
        .overlay(
            RoundedRectangle.soft(Design.Radius.control)
                .strokeBorder(
                    focused ? AnyShapeStyle(Design.accent.opacity(0.55)) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
    }
}

private struct ResizeGrip: View {
    var body: some View {
        Path { path in
            path.move(to: CGPoint(x: 10, y: 2))
            path.addLine(to: CGPoint(x: 2, y: 10))
            path.move(to: CGPoint(x: 10, y: 6))
            path.addLine(to: CGPoint(x: 6, y: 10))
        }
        .stroke(Design.inkFaint, style: StrokeStyle(lineWidth: 1.5, lineCap: .round))
        .frame(width: 12, height: 12)
        .padding(Design.Space.xs)
        .contentShape(Rectangle())
    }
}

private struct InkMenuDismissKey: EnvironmentKey {
    static let defaultValue: @MainActor () -> Void = {}
}

extension EnvironmentValues {
    var inkMenuDismiss: @MainActor () -> Void {
        get { self[InkMenuDismissKey.self] }
        set { self[InkMenuDismissKey.self] = newValue }
    }
}

struct InkMenu<Content: View>: View {
    enum Trigger {
        case standard
        case chip
    }

    let title: String
    var accessibilityName: String = "menu"
    var readyDot: Bool? = nil
    var externalOpen: Binding<Bool>? = nil
    var trigger: Trigger = .standard
    var help: String? = nil
    @ViewBuilder let content: () -> Content
    @State private var localOpen = false
    @State private var hovering = false

    private var open: Binding<Bool> {
        externalOpen ?? $localOpen
    }

    private var triggerShape: AnyInsettableShape {
        switch trigger {
        case .standard: AnyInsettableShape(RoundedRectangle.soft(Design.Radius.control))
        case .chip: AnyInsettableShape(Capsule())
        }
    }

    private var triggerFill: Color {
        switch trigger {
        case .standard: hovering ? Design.inkWash : Design.surface
        case .chip: hovering ? Design.ink.opacity(0.10) : Design.inkWash
        }
    }

    private var triggerBorder: AnyShapeStyle {
        switch trigger {
        case .standard:
            open.wrappedValue
                ? AnyShapeStyle(Design.accent.opacity(0.55)) : AnyShapeStyle(Design.line)
        case .chip:
            AnyShapeStyle(Design.line)
        }
    }

    var body: some View {
        Button {
            open.wrappedValue = true
        } label: {
            HStack(spacing: Design.Space.xs) {
                if let readyDot {
                    Circle()
                        .fill(
                            readyDot
                                ? AnyShapeStyle(Design.accent) : AnyShapeStyle(Design.inkFaint)
                        )
                        .frame(width: 6, height: 6)
                }
                Text(title)
                    .font(Design.label)
                    .lineLimit(1)
                    .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                Image(systemName: "chevron.up.chevron.down")
                    .font(Design.glyphSmall)
                    .foregroundStyle(Design.inkFaint)
            }
            .padding(.horizontal, Design.Space.chipX)
            .frame(height: Design.Control.size)
            .background(triggerFill, in: triggerShape)
            .overlay(
                triggerShape.strokeBorder(triggerBorder, lineWidth: Design.hairlineWidth))
            .contentShape(triggerShape)
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(triggerShape)
        .fixedSize()
        .animation(Design.wash, value: hovering)
        .help(help ?? "")
        .popover(isPresented: open, arrowEdge: .top) {
            InkPopoverBody(
                width: Design.Popover.menuWidth, maxHeight: Design.Popover.menuMaxHeight
            ) {
                menu
            }
        }
        .accessibilityLabel(accessibilityName)
    }

    private var menu: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxs) {
            content()
        }
        .padding(Design.Space.s)
        .environment(\.inkMenuDismiss) { open.wrappedValue = false }
    }
}

struct InkMenuRow: View {
    let title: String
    var annotation: String? = nil
    var selected = false
    var disabled = false
    var previewing = false
    var onPreview: (() -> Void)? = nil
    let action: () -> Void
    @Environment(\.inkMenuDismiss) private var dismissMenu
    @State private var hovering = false

    var body: some View {
        Button {
            action()
            dismissMenu()
        } label: {
            HStack(spacing: Design.Space.s) {
                Text(title)
                    .font(Design.caption)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .foregroundStyle(disabled ? Design.inkFaint : Design.ink)
                Spacer(minLength: Design.Space.s)
                if let annotation {
                    Text(annotation)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .lineLimit(1)
                }
                if let onPreview {
                    Button(action: onPreview) {
                        Image(systemName: previewing ? "waveform" : "play.circle")
                            .font(Design.glyphInline)
                            .foregroundStyle(
                                previewing
                                    ? Design.ink
                                    : hovering ? Design.inkSoft : Design.inkFaint)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("Preview \(title)")
                    .accessibilityLabel("Preview \(title)")
                }
                if selected {
                    Image(systemName: "checkmark")
                        .font(Design.glyphSmall.weight(.bold))
                        .foregroundStyle(Design.ink)
                        .symbolEffect(.bounce, value: selected)
                }
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected
                    ? Design.ink.opacity(0.08)
                    : hovering && !disabled ? Design.ink.opacity(0.04) : .clear,
                in: RoundedRectangle.soft(Design.Radius.card))
            .contentShape(RoundedRectangle.soft(Design.Radius.card))
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .onHover { hovering = $0 }
        .accessibilityLabel(title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct InkMenuHeader: View {
    let title: String

    var body: some View {
        MicroHeader(title: title)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.top, Design.Space.s)
            .padding(.bottom, Design.Space.xxs)
    }
}

struct InkMenuDivider: View {
    var body: some View {
        Rectangle()
            .fill(Design.line)
            .frame(height: Design.hairlineWidth)
            .padding(.vertical, Design.Space.xxs)
    }
}

struct InkDropdown: View {
    let options: [String]
    let selection: String?
    var placeholder: String = "auto"
    var allowsAuto = true
    var accessibilityName: String = "option"
    var width: CGFloat? = nil
    var size: InkControlSize = .compact
    var rowFont: ((String) -> Font)? = nil
    var onPreview: ((String) -> Void)? = nil
    let onSelect: (String?) -> Void
    @State private var open = false
    @State private var hovering = false

    var body: some View {
        Button {
            open = true
        } label: {
            HStack(spacing: Design.Space.s) {
                Text(selection ?? placeholder)
                    .font(size == .settings ? Design.body : Design.caption)
                    .lineLimit(1)
                    .foregroundStyle(selection == nil ? Design.inkFaint : Design.ink)
                Spacer(minLength: Design.Space.m)
                Image(systemName: "chevron.up.chevron.down")
                    .font(Design.glyphSmall)
                    .foregroundStyle(Design.inkFaint)
            }
            .padding(.horizontal, size == .settings ? Design.Space.l : Design.Space.chipX)
            .padding(.vertical, size == .settings ? 0 : Design.Space.s)
            .frame(height: size == .settings ? Design.Control.fieldHeight : nil)
            .background(
                hovering
                    ? Design.inkWash
                    : size == .settings ? Design.surface2 : Design.surface,
                in: RoundedRectangle.soft(Design.Radius.control))
            .overlay(
                RoundedRectangle.soft(Design.Radius.control).strokeBorder(
                    open ? AnyShapeStyle(Design.accent.opacity(0.55)) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
            .frame(
                maxWidth: size == .settings && width == nil ? Design.Control.fieldWidth : nil)
            .frame(width: width)
            .fixedSize(horizontal: width == nil, vertical: true)
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .inkFocusRing(RoundedRectangle.soft(Design.Radius.control))
        .popover(isPresented: $open, arrowEdge: .top) {
            InkPopoverBody(
                width: Design.Popover.dropdownWidth, maxHeight: Design.Popover.dropdownMaxHeight
            ) {
                rows
            }
        }
        .accessibilityLabel("Choose \(accessibilityName)")
    }

    private var rows: some View {
        ScrollViewReader { proxy in
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                if allowsAuto {
                    dropdownRow(title: placeholder, value: nil, faint: true)
                    Rectangle()
                        .fill(Design.line)
                        .frame(height: Design.hairlineWidth)
                        .padding(.vertical, Design.Space.xxs)
                }
                ForEach(options, id: \.self) { candidate in
                    dropdownRow(title: candidate, value: candidate, faint: false)
                        .id(candidate)
                }
            }
            .padding(Design.Space.s)
            .onAppear {
                if let selection { proxy.scrollTo(selection, anchor: .center) }
            }
        }
    }

    private func dropdownRow(title: String, value: String?, faint: Bool) -> some View {
        DropdownRow(
            title: title,
            selected: selection == value,
            faint: faint,
            font: value.flatMap { rowFont?($0) } ?? Design.caption,
            onPreview: value.flatMap { candidate in
                onPreview.map { preview in { preview(candidate) } }
            }
        ) {
            onSelect(value)
            open = false
        }
    }
}

private struct DropdownRow: View {
    let title: String
    let selected: Bool
    let faint: Bool
    var font: Font = Design.caption
    var onPreview: (() -> Void)? = nil
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Design.Space.s) {
                Text(title)
                    .font(font)
                    .lineLimit(1)
                    .foregroundStyle(faint ? Design.inkSoft : Design.ink)
                Spacer(minLength: Design.Space.s)
                if let onPreview {
                    Button(action: onPreview) {
                        Image(systemName: "play.circle")
                            .font(Design.glyphInline)
                            .foregroundStyle(hovering ? Design.inkSoft : Design.inkFaint)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("Preview \(title)")
                    .accessibilityLabel("Preview \(title)")
                }
                if selected {
                    Image(systemName: "checkmark")
                        .font(Design.glyphSmall.weight(.bold))
                        .foregroundStyle(Design.ink)
                        .symbolEffect(.bounce, value: selected)
                }
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected
                    ? Design.ink.opacity(0.08)
                    : hovering ? Design.ink.opacity(0.04) : .clear,
                in: RoundedRectangle.soft(Design.Radius.card))
            .contentShape(RoundedRectangle.soft(Design.Radius.card))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(title)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct QuietButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Design.caption.weight(.medium))
            .foregroundStyle(Design.ink)
            .padding(.horizontal, Design.Space.l)
            .padding(.vertical, Design.Space.xs + 1)
            .background(
                hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.surface),
                in: RoundedRectangle.soft(Design.Radius.control))
            .overlay(
                RoundedRectangle.soft(Design.Radius.control).strokeBorder(
                    hovering ? AnyShapeStyle(Design.inkFaint) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
            .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
            .onHover { hovering = $0 }
            .animation(Design.wash, value: hovering)
    }
}

struct AnyInsettableShape: InsettableShape {
    private let pathBuilder: @Sendable (CGRect, CGFloat) -> Path

    init<S: InsettableShape>(_ shape: S) {
        pathBuilder = { rect, inset in
            shape.inset(by: inset).path(in: rect)
        }
    }

    func path(in rect: CGRect) -> Path {
        pathBuilder(rect, 0)
    }

    func inset(by amount: CGFloat) -> AnyInsettableShape {
        let builder = pathBuilder
        var copy = self
        copy = AnyInsettableShape(offset: amount, builder: builder)
        return copy
    }

    private init(offset: CGFloat, builder: @escaping @Sendable (CGRect, CGFloat) -> Path) {
        pathBuilder = { rect, inset in
            builder(rect, inset + offset)
        }
    }
}

extension Comparable {
    func clamped(to range: ClosedRange<Self>) -> Self {
        min(max(self, range.lowerBound), range.upperBound)
    }
}

struct InkChoiceCard<Preview: View>: View {
    let label: String
    let selected: Bool
    let action: () -> Void
    @ViewBuilder let preview: () -> Preview
    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button(action: action) {
            VStack(spacing: Design.Space.s) {
                preview()
                    .frame(width: 96, height: 62)
                    .clipShape(RoundedRectangle.soft(Design.Radius.control))
                    .overlay(
                        RoundedRectangle.soft(Design.Radius.control)
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                Text(label)
                    .font(Design.label.weight(selected ? .semibold : .regular))
                    .foregroundStyle(selected ? Design.ink : Design.inkSoft)
            }
            .padding(Design.Space.s)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.control))
            .overlay(
                RoundedRectangle.soft(Design.Radius.control)
                    .strokeBorder(
                        selected ? AnyShapeStyle(Design.accent) : AnyShapeStyle(Design.line),
                        lineWidth: selected ? 1.5 : Design.hairlineWidth))
            .offset(y: hovering && !reduceMotion ? -2 : 0)
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
            .animation(Design.wash, value: hovering)
            .animation(Design.wash, value: selected)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct ThemeFamilyCard: View {
    let family: ThemeFamily
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Button(action: action) {
            VStack(spacing: Design.Space.s) {
                ThemePreview(family: family, variant: .system)
                    .frame(maxWidth: .infinity)
                    .frame(height: 108)
                    .clipShape(RoundedRectangle.soft(Design.Radius.card))
                    .overlay(
                        RoundedRectangle.soft(Design.Radius.card)
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                HStack(spacing: Design.Space.xs) {
                    Text(family.name)
                        .font(Design.body.weight(selected ? .semibold : .regular))
                        .foregroundStyle(selected ? Design.ink : Design.inkSoft)
                    Spacer()
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 13))
                        .foregroundStyle(selected ? Design.accent : Design.inkFaint)
                }
                .padding(.horizontal, Design.Space.xxs)
            }
            .padding(Design.Space.s)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(
                        selected ? AnyShapeStyle(Design.accent) : AnyShapeStyle(Design.line),
                        lineWidth: selected ? 1.5 : Design.hairlineWidth))
            .offset(y: hovering && !reduceMotion ? -2 : 0)
            .contentShape(RoundedRectangle.soft(Design.Radius.card))
            .animation(Design.wash, value: hovering)
            .animation(Design.wash, value: selected)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(family.name)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct ThemePreview: View {
    enum Variant {
        case system
        case light
        case dark
    }

    var family: ThemeFamily = .standard
    let variant: Variant

    private func swatch(_ palette: ThemePalette) -> (Color, Color, Color, Color, Color) {
        (
            Design.fixed(palette.ground), Design.fixed(palette.card), Design.fixed(palette.text),
            Design.fixed(palette.muted), Design.fixed(palette.accentDim)
        )
    }

    var body: some View {
        let l = swatch(family.light)
        let d = swatch(family.dark)
        switch variant {
        case .light:
            mock(paper: l.0, surface: l.1, ink: l.2, soft: l.3, accent: l.4)
        case .dark:
            mock(paper: d.0, surface: d.1, ink: d.2, soft: d.3, accent: d.4)
        case .system:
            ZStack {
                mock(paper: l.0, surface: l.1, ink: l.2, soft: l.3, accent: l.4)
                mock(paper: d.0, surface: d.1, ink: d.2, soft: d.3, accent: d.4)
                    .clipShape(DiagonalHalf())
            }
        }
    }

    private func mock(
        paper: Color, surface: Color, ink: Color, soft: Color, accent: Color
    ) -> some View {
        ZStack(alignment: .topLeading) {
            paper
            HStack(spacing: 4) {
                VStack(alignment: .leading, spacing: 3) {
                    Circle()
                        .fill(accent)
                        .frame(width: 5, height: 5)
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(soft.opacity(0.55))
                        .frame(width: 14, height: 3)
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(soft.opacity(0.35))
                        .frame(width: 12, height: 3)
                    Spacer()
                }
                .padding(4)
                .frame(width: 24)
                .background(
                    RoundedRectangle(cornerRadius: 2).fill(surface))
                .padding(.vertical, 4)
                VStack(alignment: .leading, spacing: 4) {
                    RoundedRectangle.soft(Design.Radius.control)
                        .fill(ink.opacity(0.85))
                        .frame(width: 26, height: 6)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                    VStack(alignment: .leading, spacing: 2) {
                        RoundedRectangle(cornerRadius: 1.5)
                            .fill(soft.opacity(0.55))
                            .frame(width: 34, height: 3)
                        RoundedRectangle(cornerRadius: 1.5)
                            .fill(soft.opacity(0.4))
                            .frame(width: 28, height: 3)
                    }
                    Spacer()
                    RoundedRectangle.soft(Design.Radius.control)
                        .fill(surface)
                        .overlay(RoundedRectangle.soft(Design.Radius.control).strokeBorder(soft.opacity(0.3), lineWidth: 0.5))
                        .frame(height: 8)
                        .overlay(alignment: .trailing) {
                            Circle()
                                .fill(ink)
                                .frame(width: 5, height: 5)
                                .padding(.trailing, 2)
                        }
                }
                .padding(.vertical, 5)
                .padding(.trailing, 5)
            }
            .padding(.leading, 4)
        }
    }
}

private struct DiagonalHalf: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.maxX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY))
        path.closeSubpath()
        return path
    }
}

struct WidthPreview: View {
    let wide: Bool

    var body: some View {
        ZStack {
            Design.paper
            VStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { index in
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(Design.inkSoft.opacity(0.5))
                        .frame(
                            width: wide ? 60 : 36,
                            height: 4)
                        .frame(maxWidth: .infinity)
                        .opacity(index == 2 ? 0.6 : 1)
                }
            }
            .padding(.horizontal, 6)
        }
    }
}

struct DensityPreview: View {
    let compact: Bool

    var body: some View {
        ZStack {
            Design.paper
            VStack(spacing: compact ? 3 : 7) {
                ForEach(0..<4, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(Design.inkSoft.opacity(0.5))
                        .frame(width: 48, height: 4)
                }
            }
        }
    }
}


private struct InkTextViewRepresentable: NSViewRepresentable {
    @Binding var text: String
    var focused: FocusState<Bool>.Binding

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: InkTextViewRepresentable

        init(parent: InkTextViewRepresentable) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let view = notification.object as? NSTextView else { return }
            parent.text = view.string
        }

        func textDidBeginEditing(_ notification: Notification) {
            parent.focused.wrappedValue = true
        }

        func textDidEndEditing(_ notification: Notification) {
            parent.focused.wrappedValue = false
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.hasVerticalScroller = false
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        if let view = scroll.documentView as? NSTextView {
            view.delegate = context.coordinator
            view.font = Design.editorFont(size: NSFont.systemFontSize(for: .small) + 1)
            view.drawsBackground = false
            view.isRichText = false
            view.allowsUndo = true
            view.textContainerInset = NSSize(width: 2, height: 4)
            view.string = text
        }
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let view = scroll.documentView as? NSTextView else { return }
        if view.string != text {
            view.string = text
        }
        let inkColor = NSColor(Design.ink)
        if view.textColor != inkColor {
            view.textColor = inkColor
        }
    }
}
