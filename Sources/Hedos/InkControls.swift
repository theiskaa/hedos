import SwiftUI

struct InkSlider: View {
    let range: ClosedRange<Double>
    let value: Double
    let isSet: Bool
    let onChange: (Double) -> Void
    var label: String = "Value"
    @State private var hovering = false
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
                Capsule()
                    .fill(Design.line)
                    .frame(height: 4)
                Capsule()
                    .fill(isSet ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.inkFaint))
                    .frame(width: max(7, thumbX + 7), height: 4)
                Circle()
                    .fill(isSet ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface))
                    .overlay(
                        Circle().strokeBorder(
                            isSet ? Design.paper.opacity(0.35) : Design.inkFaint,
                            lineWidth: 1))
                    .frame(width: 14, height: 14)
                    .shadow(
                        color: Design.shadowColor.opacity(isSet ? 0.25 : 0.10),
                        radius: 3, x: 0, y: 1)
                    .scaleEffect(hovering && !reduceMotion ? 1.15 : 1)
                    .offset(x: thumbX)
                    .animation(.easeOut(duration: 0.15), value: hovering)
            }
            .frame(height: 18)
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { gesture in
                        let usable = max(1, width - 14)
                        let raw = ((gesture.location.x - 7) / usable).clamped(to: 0...1)
                        onChange(
                            range.lowerBound + raw * (range.upperBound - range.lowerBound))
                    })
            .onHover { hovering = $0 }
        }
        .frame(height: 18)
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
                Capsule()
                    .fill(
                        isOn && isSet
                            ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface))
                Capsule()
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
            .contentShape(Capsule())
            .animation(
                reduceMotion ? nil : .easeOut(duration: 0.15), value: isOn)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityValue(isSet ? (isOn ? "on" : "off") : "auto")
        .accessibilityAddTraits(.isToggle)
    }
}

struct InkSegmented: View {
    let values: [String]
    let selection: String?
    let onSelect: (String) -> Void

    var body: some View {
        HStack(spacing: Design.Space.xs) {
            ForEach(values, id: \.self) { candidate in
                Button {
                    onSelect(candidate)
                } label: {
                    Text(candidate)
                        .font(Design.label.weight(selection == candidate ? .semibold : .regular))
                        .lineLimit(1)
                        .fixedSize()
                        .foregroundStyle(
                            selection == candidate ? Design.paper : Design.inkSoft)
                        .padding(.horizontal, Design.Space.m)
                        .padding(.vertical, Design.Space.xs)
                        .background(
                            selection == candidate
                                ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface),
                            in: Capsule())
                        .overlay(
                            Capsule().strokeBorder(
                                selection == candidate ? .clear : Design.line,
                                lineWidth: Design.hairlineWidth))
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(candidate)
                .accessibilityAddTraits(selection == candidate ? .isSelected : [])
            }
        }
    }
}

struct InkField: View {
    enum FieldShape {
        case rounded
        case capsule
    }

    let placeholder: String
    @Binding var text: String
    var shape: FieldShape = .rounded
    var font: Font = Design.caption
    var onSubmit: (() -> Void)? = nil
    var onFocusLost: (() -> Void)? = nil
    @FocusState private var focused: Bool

    var body: some View {
        TextField("", text: $text, prompt: Text(placeholder).foregroundStyle(Design.inkFaint))
            .textFieldStyle(.plain)
            .font(font)
            .foregroundStyle(Design.ink)
            .focused($focused)
            .onSubmit { onSubmit?() }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.xs + 1)
            .background(Design.surface, in: fieldShape)
            .overlay(
                fieldShape.strokeBorder(
                    focused ? AnyShapeStyle(Design.ink.opacity(0.35)) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
            .onChange(of: focused) { _, isFocused in
                if !isFocused { onFocusLost?() }
            }
    }

    private var fieldShape: AnyInsettableShape {
        switch shape {
        case .rounded: AnyInsettableShape(RoundedRectangle(cornerRadius: Design.Radius.inner))
        case .capsule: AnyInsettableShape(Capsule())
        }
    }
}

struct InkTextArea: View {
    let placeholder: String
    @Binding var text: String
    var lines: ClosedRange<Int> = 2...6
    @FocusState private var focused: Bool

    var body: some View {
        TextField(
            "", text: $text, prompt: Text(placeholder).foregroundStyle(Design.inkFaint),
            axis: .vertical
        )
        .textFieldStyle(.plain)
        .font(Design.caption)
        .foregroundStyle(Design.ink)
        .lineLimit(lines.lowerBound...lines.upperBound)
        .focused($focused)
        .padding(.horizontal, Design.Space.chipX)
        .padding(.vertical, Design.Space.s)
        .background(Design.surface, in: RoundedRectangle(cornerRadius: Design.Radius.inner))
        .overlay(
            RoundedRectangle(cornerRadius: Design.Radius.inner)
                .strokeBorder(
                    focused ? AnyShapeStyle(Design.ink.opacity(0.35)) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
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
                in: Capsule())
            .overlay(
                Capsule().strokeBorder(
                    hovering ? AnyShapeStyle(Design.inkFaint) : AnyShapeStyle(Design.line),
                    lineWidth: Design.hairlineWidth))
            .opacity(isEnabled ? (configuration.isPressed ? 0.7 : 1) : 0.4)
            .contentShape(Capsule())
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.15), value: hovering)
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
