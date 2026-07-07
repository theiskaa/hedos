import AppKit
import HedosKernel
import SwiftUI

enum Design {
    enum Space {
        static let xxs: CGFloat = 2
        static let xs: CGFloat = 4
        static let s: CGFloat = 6
        static let m: CGFloat = 8
        static let chipX: CGFloat = 10
        static let l: CGFloat = 12
        static let tile: CGFloat = 14
        static let xl: CGFloat = 16
        static let xxl: CGFloat = 20
        static let gutter: CGFloat = 24
        static let pane: CGFloat = 28
    }

    enum Radius {
        static let card: CGFloat = 8
        static let inner: CGFloat = 10
        static let bubble: CGFloat = 14
        static let tile: CGFloat = 14
        static let surface: CGFloat = 20
    }

    enum Rail {
        static let columnWidth: CGFloat = 248
    }

    static let conversationMaxWidth: CGFloat = 840

    static let paneTitle = Font.title.weight(.semibold)
    static let title = Font.title3.weight(.semibold)
    static let body = Font.body
    static let bodyLineSpacing: CGFloat = 3.5
    static let caption = Font.callout
    static let label = Font.caption
    static let micro = Font.system(size: 11, weight: .medium, design: .monospaced)
    static let microTracking: CGFloat = 1.3
    static let tightTracking: CGFloat = -0.3

    static let glyphNav = Font.system(size: 16, weight: .medium)
    static let glyphPrimary = Font.system(size: 15, weight: .medium)
    static let glyphInline = Font.system(size: 11)
    static let glyphSmall = Font.system(size: 9, weight: .semibold)
    static let glyphMicro = Font.system(size: 7, weight: .semibold)

    static func plaque(_ size: CGFloat, weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func data(_ size: CGFloat) -> Font {
        .system(size: size, weight: .regular, design: .monospaced)
    }

    static func markdownHeading(_ level: Int) -> Font {
        switch level {
        case 1: .title2.weight(.semibold)
        case 2: .title3.weight(.semibold)
        case 3: .headline
        default: .body.weight(.semibold)
        }
    }

    static let spring = Animation.spring(response: 0.35, dampingFraction: 0.8)

    static func motion(reduceMotion: Bool) -> Animation? {
        reduceMotion ? nil : spring
    }

    static let paper = warm(light: 0xF1EFEC, dark: 0x1E1D1B)
    static let surface = warm(light: 0xFAF9F7, dark: 0x262523)
    static let line = warm(light: 0xE2DFDA, dark: 0x3A3835)
    static let ink = warm(light: 0x2B2A28, dark: 0xE8E6E1)
    static let inkSoft = warm(light: 0x6F6C66, dark: 0xA29E96)
    static let inkFaint = warm(light: 0x9B978F, dark: 0x6E6A63)
    static let inkWash = ink.opacity(0.06)

    private static func warm(light: Int, dark: Int) -> Color {
        Color(
            nsColor: NSColor(
                name: nil,
                dynamicProvider: { appearance in
                    let hex =
                        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                        ? dark : light
                    return NSColor(
                        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                        green: CGFloat((hex >> 8) & 0xFF) / 255,
                        blue: CGFloat(hex & 0xFF) / 255,
                        alpha: 1)
                }))
    }

    static let cardFill = AnyShapeStyle(surface)
    static let bubbleFill = AnyShapeStyle(inkWash)
    static let tableFill = AnyShapeStyle(surface)
    static let hairline = AnyShapeStyle(line)
    static let hairlineWidth: CGFloat = 1
    static let ruleWidth: CGFloat = 2

    static let shadowColor = Color(red: 43 / 255, green: 42 / 255, blue: 40 / 255)

    static func modalityGlyph(_ modality: Modality) -> String {
        switch modality {
        case .text: "text.alignleft"
        case .speech: "waveform"
        case .audio: "ear"
        case .image: "photo"
        default: "shippingbox"
        }
    }

    static func modeGlyph(_ mode: AppMode) -> String {
        switch mode {
        case .chat: "bubble.left"
        case .images: "photo"
        case .voice: "waveform"
        case .library: "books.vertical"
        case .settings: "gearshape"
        }
    }

    static func modeTitle(_ mode: AppMode) -> String {
        switch mode {
        case .chat: "Chat"
        case .images: "Images"
        case .voice: "Voice"
        case .library: "Models"
        case .settings: "Settings"
        }
    }

    static func tagGlyph(_ tag: String) -> String? {
        switch tag {
        case SessionTag.thinking: "brain"
        case SessionTag.spoke: "waveform"
        case SessionTag.generatedImage: "photo"
        default: nil
        }
    }
}

struct ModalScrim<Modal: View>: ViewModifier {
    let isPresented: Bool
    let onDismiss: () -> Void
    @ViewBuilder let modal: () -> Modal

    func body(content: Content) -> some View {
        content.overlay {
            if isPresented {
                ZStack {
                    Design.shadowColor.opacity(0.22)
                        .ignoresSafeArea()
                        .onTapGesture(perform: onDismiss)
                        .accessibilityLabel("Dismiss")
                    modal()
                        .background(
                            Design.paper,
                            in: RoundedRectangle(cornerRadius: Design.Radius.surface))
                        .clipShape(RoundedRectangle(cornerRadius: Design.Radius.surface))
                        .overlay(
                            RoundedRectangle(cornerRadius: Design.Radius.surface)
                                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                        .shadow(
                            color: Design.shadowColor.opacity(0.30), radius: 40, x: 0, y: 18)
                }
                .onExitCommand(perform: onDismiss)
                .transition(.opacity)
            }
        }
    }
}

extension View {
    func modalScrim<Modal: View>(
        isPresented: Bool, onDismiss: @escaping () -> Void,
        @ViewBuilder modal: @escaping () -> Modal
    ) -> some View {
        modifier(ModalScrim(isPresented: isPresented, onDismiss: onDismiss, modal: modal))
    }
}

struct SurfaceCard: ViewModifier {
    var radius: CGFloat = Design.Radius.surface

    func body(content: Content) -> some View {
        content
            .background(Design.surface, in: RoundedRectangle(cornerRadius: radius))
            .overlay(
                RoundedRectangle(cornerRadius: radius)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: radius))
    }
}

struct Lifts: ViewModifier {
    let hovering: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .shadow(
                color: Design.shadowColor.opacity(hovering ? 0.18 : 0.12),
                radius: hovering ? 30 : 24,
                x: 0,
                y: hovering ? 14 : 10)
            .offset(y: hovering && !reduceMotion ? -3 : 0)
            .animation(.easeOut(duration: 0.2), value: hovering)
    }
}

extension View {
    func surfaceCard(radius: CGFloat = Design.Radius.surface) -> some View {
        modifier(SurfaceCard(radius: radius))
    }

    func lifts(hovering: Bool) -> some View {
        modifier(Lifts(hovering: hovering))
    }

    func tile(hovering: Bool = false) -> some View {
        surfaceCard(radius: Design.Radius.tile)
            .lifts(hovering: hovering)
    }
}

struct InkButtonStyle: ButtonStyle {
    var circle = false
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Design.body.weight(.medium))
            .foregroundStyle(Design.paper)
            .padding(.horizontal, circle ? 0 : Design.Space.xl)
            .padding(.vertical, circle ? 0 : Design.Space.s + 1)
            .frame(width: circle ? 28 : nil, height: circle ? 28 : nil)
            .background(Design.ink, in: Capsule())
            .overlay(
                Capsule()
                    .strokeBorder(
                        LinearGradient(
                            colors: [
                                Color.white.opacity(colorScheme == .dark ? 0.10 : 0.18),
                                .clear,
                            ],
                            startPoint: .top, endPoint: .center),
                        lineWidth: 1))
            .shadow(
                color: Design.shadowColor.opacity(
                    configuration.isPressed ? 0.14 : hovering ? 0.30 : 0.22),
                radius: hovering && !configuration.isPressed ? 18 : 12,
                x: 0,
                y: configuration.isPressed ? 4 : hovering ? 9 : 6)
            .offset(y: configuration.isPressed ? 0 : hovering ? -1 : 0)
            .opacity(isEnabled ? 1 : 0.4)
            .contentShape(Capsule())
            .onHover { hovering = $0 }
            .animation(.easeOut(duration: 0.2), value: hovering)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

struct FilterChip: View {
    let label: String
    let isOn: Bool
    var mark: SourceKind? = nil
    let action: () -> Void

    init(
        label: String, isOn: Bool, mark: SourceKind? = nil,
        action: @escaping () -> Void
    ) {
        self.label = label
        self.isOn = isOn
        self.mark = mark
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: Design.Space.xs) {
                if isOn {
                    Image(systemName: "checkmark")
                        .font(Design.glyphSmall.weight(.bold))
                }
                if let mark {
                    SourceMark(kind: mark, size: 12)
                }
                Text(label)
                    .font(Design.caption.weight(isOn ? .semibold : .medium))
            }
            .foregroundStyle(isOn ? Design.paper : Design.inkSoft)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .background(
                isOn ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface),
                in: Capsule())
            .overlay(
                Capsule()
                    .strokeBorder(
                        isOn ? AnyShapeStyle(.clear) : Design.hairline,
                        lineWidth: Design.hairlineWidth))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

struct MicroHeader: View {
    let title: String

    var body: some View {
        Text(title.uppercased())
            .font(Design.micro)
            .tracking(Design.microTracking)
            .foregroundStyle(Design.inkFaint)
    }
}

struct QuietDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Button {
                configuration.isExpanded.toggle()
            } label: {
                HStack(spacing: Design.Space.xs) {
                    configuration.label
                    Image(
                        systemName: configuration.isExpanded ? "chevron.down" : "chevron.right"
                    )
                    .font(Design.glyphMicro)
                    .foregroundStyle(Design.inkFaint)
                }
            }
            .buttonStyle(.plain)
            if configuration.isExpanded {
                configuration.content
            }
        }
    }
}

struct LeftRule: ViewModifier {
    func body(content: Content) -> some View {
        content
            .padding(.leading, 10)
            .overlay(alignment: .leading) {
                Rectangle()
                    .fill(Design.hairline)
                    .frame(width: Design.ruleWidth)
            }
    }
}

extension View {
    func leftRule() -> some View {
        modifier(LeftRule())
    }

}

struct HeptagonMark: View {
    var size: CGFloat
    var color: Color = Design.ink

    var body: some View {
        HeptagonShape()
            .stroke(color, style: StrokeStyle(lineWidth: size * 0.062, lineJoin: .round))
            .fill(color)
            .frame(width: size, height: size)
    }
}

struct HeptagonShape: Shape {
    func path(in rect: CGRect) -> Path {
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) * 0.44
        var path = Path()
        for index in 0..<7 {
            let angle = (Double(index) * 2 * .pi / 7) - .pi / 2
            let point = CGPoint(
                x: center.x + radius * cos(angle),
                y: center.y + radius * sin(angle))
            if index == 0 {
                path.move(to: point)
            } else {
                path.addLine(to: point)
            }
        }
        path.closeSubpath()
        return path
    }
}

struct SpeakingIndicator: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var phase = false

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { index in
                Capsule()
                    .fill(Design.inkSoft)
                    .frame(width: 2.5, height: phase ? barHeight(index) : 4)
                    .animation(
                        reduceMotion
                            ? nil
                            : .easeInOut(duration: 0.45)
                                .repeatForever(autoreverses: true)
                                .delay(Double(index) * 0.09),
                        value: phase)
            }
        }
        .frame(height: 16)
        .onAppear { phase = true }
        .accessibilityLabel("Speaking")
    }

    private func barHeight(_ index: Int) -> CGFloat {
        [9, 14, 16, 12, 8][index]
    }
}
