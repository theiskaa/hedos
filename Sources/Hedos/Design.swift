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
        static var control: CGFloat { ThemeStore.shared.shape.control }
        static var card: CGFloat { ThemeStore.shared.shape.card }
        static var tile: CGFloat { ThemeStore.shared.shape.tile }
        static var surface: CGFloat { ThemeStore.shared.shape.surface }
        static var bubble: CGFloat { ThemeStore.shared.shape.bubble }
        static var artifact: CGFloat { ThemeStore.shared.shape.artifact }
    }

    struct Shade {
        let opacity: Double
        let radius: CGFloat
        let y: CGFloat
    }

    enum Elevation {
        static let lift = Shade(opacity: 0.12, radius: 24, y: 10)
        static let liftHover = Shade(opacity: 0.18, radius: 30, y: 14)
        static let floating = Shade(opacity: 0.18, radius: 24, y: 10)
        static let button = Shade(opacity: 0.22, radius: 12, y: 6)
        static let buttonHover = Shade(opacity: 0.30, radius: 18, y: 9)
        static let modal = Shade(opacity: 0.30, radius: 40, y: 18)
    }

    enum Rail {
        static let columnWidth: CGFloat = 248
        static let expandedWidth: CGFloat = 224
        static let collapsedWidth: CGFloat = 84
    }

    enum Window {
        static let mainMin = CGSize(width: 860, height: 520)
        static let settings = CGSize(width: 920, height: 560)
        static let settingsMin = CGSize(width: 760, height: 520)
        static let aboutWidth: CGFloat = 300
    }

    enum Sheet {
        static let gallery = CGSize(width: 640, height: 520)
        static let modelDetailWidth: CGFloat = 600
        static let modelDetailHeight: CGFloat = 680
        static let modelRecipeHeight: CGFloat = 560
        static let promptWidth: CGFloat = 500
        static let promptHeight: CGFloat = 560
        static let serverWidth: CGFloat = 480
        static let serverHeight: CGFloat = 520
    }

    enum Column {
        static let settingsDetail: CGFloat = 640
        static let control: CGFloat = 220
        static let hero: CGFloat = 780
        static let prose: CGFloat = 520
        static let transcriptProse: CGFloat = 620
        static let emptyCaption: CGFloat = 380
        static let pipelineList: CGFloat = 300
    }

    enum Popover {
        static let menuWidth: CGFloat = 250
        static let menuMaxHeight: CGFloat = 300
        static let dropdownWidth: CGFloat = 200
        static let dropdownMaxHeight: CGFloat = 240
        static let paramsWidth: CGFloat = 280
        static let form = CGSize(width: 300, height: 340)
    }

    enum Bubble {
        static let promptMax: CGFloat = 520
        static let imageMax: CGFloat = 360
    }

    static let conversationMaxWidth: CGFloat = 840
    static let conversationWideWidth: CGFloat = 1040

    static let wash = Animation.easeOut(duration: 0.18)

    struct FontBook: Equatable {
        var uiFamily: String?
        var monoFamily: String?

        var identity: String {
            "\(uiFamily ?? "system")/\(monoFamily ?? "system")"
        }
    }

    nonisolated(unsafe) static var fontBook = FontBook()

    private static func ui(
        _ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle
    ) -> Font {
        guard let family = fontBook.uiFamily else {
            return .system(
                size: scaledSize(size, relativeTo: style), weight: weight, design: .monospaced)
        }
        return .custom(family, size: size, relativeTo: style).weight(weight)
    }

    private static func uiStyled(
        _ style: Font.TextStyle, size: CGFloat, weight: Font.Weight = .regular
    ) -> Font {
        guard let family = fontBook.uiFamily else {
            return .system(
                size: scaledSize(size, relativeTo: style), weight: weight, design: .monospaced)
        }
        return .custom(family, size: size, relativeTo: style).weight(weight)
    }

    private static func reading(
        _ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle
    ) -> Font {
        .system(size: scaledSize(size, relativeTo: style), weight: weight)
    }

    static var readingBody: Font { reading(14, relativeTo: .body) }
    static let readingLineSpacing: CGFloat = 4

    private static func mono(
        _ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle
    ) -> Font {
        guard let family = fontBook.monoFamily else {
            return .system(
                size: scaledSize(size, relativeTo: style), weight: weight, design: .monospaced)
        }
        return .custom(family, size: size, relativeTo: style).weight(weight)
    }

    static func scaledSize(_ base: CGFloat, relativeTo style: Font.TextStyle) -> CGFloat {
        let (nsStyle, baseline) = anchor(for: style)
        let current = NSFont.preferredFont(forTextStyle: nsStyle).pointSize
        guard baseline > 0, current > 0 else { return base }
        return (base * current / baseline).rounded()
    }

    private static func anchor(for style: Font.TextStyle) -> (NSFont.TextStyle, CGFloat) {
        switch style {
        case .largeTitle: (.largeTitle, 26)
        case .title: (.title1, 22)
        case .title2: (.title2, 17)
        case .title3: (.title3, 15)
        case .headline: (.headline, 13)
        case .callout: (.callout, 12)
        case .caption, .caption2: (.caption1, 10)
        default: (.body, 13)
        }
    }

    static var display: Font { ui(40, .semibold, relativeTo: .largeTitle) }
    static var hero: Font { ui(34, .semibold, relativeTo: .largeTitle) }
    static var heroBody: Font { ui(16, relativeTo: .body) }
    static var paneTitle: Font { uiStyled(.title, size: 22, weight: .semibold) }
    static var title: Font { uiStyled(.title3, size: 15, weight: .semibold) }
    static var body: Font { uiStyled(.body, size: 13) }
    static let bodyLineSpacing: CGFloat = 3.5
    static var caption: Font { uiStyled(.callout, size: 12) }
    static var label: Font { uiStyled(.caption, size: 10) }
    static var micro: Font { mono(11, .medium, relativeTo: .caption) }
    static let microTracking: CGFloat = 1.3
    static let tightTracking: CGFloat = -0.3

    static var glyphNav: Font { .system(size: scaledSize(16, relativeTo: .body), weight: .medium) }
    static var glyphPrimary: Font {
        .system(size: scaledSize(15, relativeTo: .body), weight: .medium)
    }
    static var glyphInline: Font { .system(size: scaledSize(11, relativeTo: .caption)) }
    static var glyphSmall: Font {
        .system(size: scaledSize(9, relativeTo: .caption), weight: .semibold)
    }
    static var glyphMicro: Font {
        .system(size: scaledSize(7, relativeTo: .caption), weight: .semibold)
    }

    static func plaque(_ size: CGFloat, weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func data(_ size: CGFloat) -> Font {
        mono(size, relativeTo: .body)
    }

    static func markdownHeading(_ level: Int) -> Font {
        switch level {
        case 1: uiStyled(.title2, size: 17, weight: .semibold)
        case 2: uiStyled(.title3, size: 15, weight: .semibold)
        case 3: uiStyled(.headline, size: 13, weight: .semibold)
        default: uiStyled(.body, size: 13, weight: .semibold)
        }
    }

    static let spring = Animation.spring(response: 0.4, dampingFraction: 0.8)

    static func motion(reduceMotion: Bool) -> Animation? {
        reduceMotion ? nil : spring
    }

    static let paper = adaptive { $0.ground }
    static let panel = adaptive { $0.panel }
    static let surface = adaptive { $0.card }
    static let surface2 = adaptive { $0.card2 }
    static let line = adaptive { $0.line }
    static let lineBright = adaptive { $0.lineBright }
    static let ink = adaptive { $0.text }
    static let inkSoft = adaptive { $0.muted }
    static let inkFaint = adaptive { $0.faint }
    static let inkWash = ink.opacity(0.06)
    static let accent = adaptive { $0.accent }
    static let accentText = adaptive { $0.accentDim }
    static let accentWash = accent.opacity(0.11)
    static let accentEdge = accent.opacity(0.25)
    static let onAccent = adaptive { $0.onAccent }
    static let heat = adaptive { $0.heat }
    static let heatText = adaptive { $0.heat }
    static let heatWash = heat.opacity(0.16)
    static let heatEdge = heat.opacity(0.32)
    static let danger = adaptive { $0.error }

    enum PreviewPalette {
        static let lightPaper = fixed(Theme.paper.palette.ground)
        static let lightSurface = fixed(Theme.paper.palette.card)
        static let lightInk = fixed(Theme.paper.palette.text)
        static let lightSoft = fixed(Theme.paper.palette.muted)
        static let lightAccent = fixed(Theme.paper.palette.accentDim)
        static let darkPaper = fixed(Theme.graphite.palette.ground)
        static let darkSurface = fixed(Theme.graphite.palette.card)
        static let darkInk = fixed(Theme.graphite.palette.text)
        static let darkSoft = fixed(Theme.graphite.palette.muted)
        static let darkAccent = fixed(Theme.graphite.palette.accentDim)
    }

    private static func fixed(_ hex: Int) -> Color {
        Color(
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255)
    }

    private static func adaptive(_ pick: @escaping @Sendable (ThemePalette) -> Int) -> Color {
        Color(
            nsColor: NSColor(
                name: nil,
                dynamicProvider: { appearance in
                    let palette =
                        appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
                        ? ThemeStore.shared.dark : ThemeStore.shared.light
                    let hex = pick(palette)
                    return NSColor(
                        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                        green: CGFloat((hex >> 8) & 0xFF) / 255,
                        blue: CGFloat(hex & 0xFF) / 255,
                        alpha: 1)
                }))
    }

    static func editorFont(size: CGFloat = NSFont.systemFontSize) -> NSFont {
        guard let family = fontBook.uiFamily,
            let font = NSFont(name: family, size: size)
                ?? NSFontManager.shared.font(
                    withFamily: family, traits: [], weight: 5, size: size)
        else { return .systemFont(ofSize: size) }
        return font
    }

    static let cardFill = AnyShapeStyle(surface)
    static let bubbleFill = AnyShapeStyle(inkWash)
    static let tableFill = AnyShapeStyle(surface)
    static let hairline = AnyShapeStyle(line)
    static let hairlineWidth: CGFloat = 1
    static let ruleWidth: CGFloat = 2

    static let shadowColor = fixed(0x0B0D10)

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
        case .home: "house"
        case .chat: "message"
        case .images: "photo.stack"
        case .voice: "speaker.wave.2"
        case .pipelines: "point.3.connected.trianglepath.dotted"
        case .library: "square.stack.3d.up"
        case .gateway: "network"
        case .settings: "gearshape"
        }
    }

    static func modeTitle(_ mode: AppMode) -> String {
        switch mode {
        case .home: "Home"
        case .chat: "Chat"
        case .images: "Images"
        case .voice: "Voice"
        case .pipelines: "Pipelines"
        case .library: "Models"
        case .gateway: "Gateway"
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
            Group {
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
                            .shade(Design.Elevation.modal)
                            .padding(Design.Space.xxl)
                    }
                    .onExitCommand(perform: onDismiss)
                    .transition(.opacity)
                }
            }
            .animation(.easeOut(duration: 0.18), value: isPresented)
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
            .shade(hovering ? Design.Elevation.liftHover : Design.Elevation.lift)
            .offset(y: hovering && !reduceMotion ? -3 : 0)
            .animation(.easeOut(duration: 0.2), value: hovering)
    }
}

struct InkFocusRing<S: InsettableShape>: ViewModifier {
    let shape: S
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        content
            .focused($focused)
            .focusEffectDisabled()
            .overlay {
                if focused {
                    shape
                        .inset(by: -2.5)
                        .stroke(Design.accent.opacity(0.55), lineWidth: Design.hairlineWidth)
                }
            }
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

    func shade(_ shade: Design.Shade) -> some View {
        shadow(
            color: Design.shadowColor.opacity(shade.opacity),
            radius: shade.radius, x: 0, y: shade.y)
    }

    func inkFocusRing<S: InsettableShape>(_ shape: S) -> some View {
        modifier(InkFocusRing(shape: shape))
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
            .background(Design.ink, in: RoundedRectangle(cornerRadius: Design.Radius.control))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .strokeBorder(
                        LinearGradient(
                            colors: [
                                Color.white.opacity(colorScheme == .dark ? 0.10 : 0.18),
                                .clear,
                            ],
                            startPoint: .top, endPoint: .center),
                        lineWidth: Design.hairlineWidth))
            .shadow(
                color: Design.shadowColor.opacity(
                    configuration.isPressed
                        ? 0.14
                        : hovering
                            ? Design.Elevation.buttonHover.opacity
                            : Design.Elevation.button.opacity),
                radius: hovering && !configuration.isPressed
                    ? Design.Elevation.buttonHover.radius : Design.Elevation.button.radius,
                x: 0,
                y: configuration.isPressed
                    ? 4
                    : hovering ? Design.Elevation.buttonHover.y : Design.Elevation.button.y)
            .offset(y: configuration.isPressed ? 0 : hovering ? -1 : 0)
            .opacity(isEnabled ? 1 : 0.4)
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
            .onHover { hovering = $0 }
            .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.control))
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

    @State private var hovering = false

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
            .foregroundStyle(isOn ? Design.paper : hovering ? Design.ink : Design.inkSoft)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .background(
                isOn
                    ? AnyShapeStyle(Design.ink)
                    : hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.surface),
                in: RoundedRectangle(cornerRadius: Design.Radius.control))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .strokeBorder(
                        isOn ? AnyShapeStyle(.clear) : Design.hairline,
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.control))
        .animation(Design.wash, value: hovering)
        .accessibilityLabel(label)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

struct PressDipStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.72 : 1)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
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

struct TintChip: View {
    let text: String
    var glyph: String? = nil
    var live = false
    var faint = false

    var body: some View {
        HStack(spacing: Design.Space.xs) {
            if let glyph {
                Image(systemName: glyph)
                    .font(Design.glyphSmall)
            }
            Text(text.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .lineLimit(1)
        }
        .foregroundStyle(
            live ? Design.accentText : faint ? Design.inkFaint : Design.inkSoft
        )
        .padding(.horizontal, Design.Space.m)
        .padding(.vertical, Design.Space.xxs + 1.5)
        .background(
            live ? AnyShapeStyle(Design.accentWash) : AnyShapeStyle(Design.inkWash),
            in: RoundedRectangle(cornerRadius: Design.Radius.control))
        .overlay(
            RoundedRectangle(cornerRadius: Design.Radius.control).strokeBorder(
                live ? AnyShapeStyle(Design.accentEdge) : AnyShapeStyle(Design.line),
                lineWidth: Design.hairlineWidth))
        .accessibilityLabel(text)
    }
}

struct FitChip: View {
    let record: ModelRecord

    var body: some View {
        if let short = Fit.short(record), let verdict = record.fit?.verdict {
            TintChip(
                text: short,
                glyph: verdict == .runsWell ? "checkmark" : verdict == .tightFit ? "minus" : nil,
                faint: verdict == .tooLarge)
        }
    }
}

struct IconPlaque<Content: View>: View {
    var size: CGFloat = 40
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
            .frame(width: size, height: size)
            .background(
                Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.control))
    }
}

struct InkRadioGroup<Value: Hashable>: View {
    let options: [(value: Value, label: String)]
    @Binding var selection: Value

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxs) {
            ForEach(Array(options.enumerated()), id: \.offset) { _, option in
                InkRadioRow(
                    label: option.label,
                    selected: selection == option.value
                ) {
                    selection = option.value
                }
            }
        }
    }
}

private struct InkRadioRow: View {
    let label: String
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Design.Space.chipX) {
                ZStack {
                    Circle()
                        .strokeBorder(
                            selected ? Design.ink : Design.inkFaint,
                            lineWidth: selected ? 1.5 : Design.hairlineWidth)
                    if selected {
                        Circle()
                            .fill(Design.ink)
                            .frame(width: 6, height: 6)
                    }
                }
                .frame(width: 14, height: 14)
                Text(label)
                    .font(Design.caption)
                    .foregroundStyle(selected || hovering ? Design.ink : Design.inkSoft)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Design.Space.m)
            .padding(.vertical, Design.Space.s)
            .background(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .fill(hovering ? Design.inkWash : .clear))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.control))
        .animation(Design.wash, value: hovering)
        .accessibilityLabel(label)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}

struct SheenBand: View {
    var tint: Color = Design.paper
    var opacity: Double = 0.85
    var duration: Double = 1.1
    @State private var phase: CGFloat = -0.7
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { geometry in
            if reduceMotion {
                Color.clear
            } else {
                LinearGradient(
                    stops: [
                        .init(color: .clear, location: 0),
                        .init(color: tint.opacity(opacity), location: 0.5),
                        .init(color: .clear, location: 1),
                    ],
                    startPoint: .leading, endPoint: .trailing)
                .frame(width: geometry.size.width * 0.7)
                .offset(x: phase * geometry.size.width)
                .onAppear {
                    phase = -0.7
                    withAnimation(.linear(duration: duration).repeatForever(autoreverses: false)) {
                        phase = 1.0
                    }
                }
            }
        }
        .allowsHitTesting(false)
    }
}

struct AccentDot: View {
    var size: CGFloat = 7
    @State private var pulsing = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        ZStack {
            if !reduceMotion {
                RoundedRectangle(cornerRadius: 1)
                    .stroke(Design.heat, lineWidth: Design.hairlineWidth)
                    .scaleEffect(pulsing ? 2.4 : 1)
                    .opacity(pulsing ? 0 : 0.8)
            }
            RoundedRectangle(cornerRadius: 1)
                .fill(Design.heat)
        }
        .frame(width: size, height: size)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeOut(duration: 1.4).repeatForever(autoreverses: false)) {
                pulsing = true
            }
        }
        .accessibilityHidden(true)
    }
}

struct ShimmerText: View {
    let text: String
    var font: Font = Design.micro
    var tracked = true

    var body: some View {
        base
            .foregroundStyle(Design.accentText)
            .overlay {
                SheenBand(tint: Design.paper, opacity: 0.7)
                    .mask(base)
            }
            .accessibilityLabel(text)
    }

    private var base: some View {
        Text(text)
            .font(font)
            .tracking(tracked ? Design.microTracking : 0)
            .lineLimit(1)
    }
}

struct SkeletonPulse: View {
    var radius: CGFloat = Design.Radius.card
    @State private var bright = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        RoundedRectangle(cornerRadius: radius)
            .fill(Design.ink.opacity(bright ? 0.09 : 0.04))
            .onAppear {
                guard !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 1.0).repeatForever(autoreverses: true)) {
                    bright = true
                }
            }
            .accessibilityHidden(true)
    }
}

struct SheetCloseButton: View {
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(Design.glyphSmall.weight(.bold))
                .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                .frame(width: 24, height: 24)
                .background(
                    hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.cardFill),
                    in: RoundedRectangle(cornerRadius: Design.Radius.control))
                .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.control))
        .keyboardShortcut(.cancelAction)
        .accessibilityLabel("Close")
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

struct LogoMark: View {
    let size: CGFloat
    var color: Color = Design.ink

    private static let artwork: NSImage? = {
        guard
            let url = Bundle.module.url(forResource: "Resources/logo", withExtension: "svg")
                ?? Bundle.module.url(forResource: "logo", withExtension: "svg"),
            let image = NSImage(contentsOf: url)
        else { return nil }
        return image
    }()

    var body: some View {
        if let artwork = Self.artwork {
            Image(nsImage: artwork)
                .resizable()
                .scaledToFit()
                .frame(width: size, height: size)
                .accessibilityHidden(true)
        } else {
            HeptagonMark(size: size, color: color)
        }
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
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .fill(Design.accent)
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

private struct ConversationWidthKey: EnvironmentKey {
    static let defaultValue: CGFloat = Design.conversationMaxWidth
}

private struct TranscriptSpacingKey: EnvironmentKey {
    static let defaultValue: CGFloat = Design.Space.xxl
}

private struct ChatShowsStatsKey: EnvironmentKey {
    static let defaultValue = true
}

private struct SendWithEnterKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    var conversationWidth: CGFloat {
        get { self[ConversationWidthKey.self] }
        set { self[ConversationWidthKey.self] = newValue }
    }

    var transcriptSpacing: CGFloat {
        get { self[TranscriptSpacingKey.self] }
        set { self[TranscriptSpacingKey.self] = newValue }
    }

    var chatShowsStats: Bool {
        get { self[ChatShowsStatsKey.self] }
        set { self[ChatShowsStatsKey.self] = newValue }
    }

    var sendWithEnter: Bool {
        get { self[SendWithEnterKey.self] }
        set { self[SendWithEnterKey.self] = newValue }
    }
}
