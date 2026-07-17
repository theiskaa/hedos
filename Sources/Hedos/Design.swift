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

    enum Control {
        static let size: CGFloat = 28
        static let fieldWidth: CGFloat = 300
        static let fieldWidthNarrow: CGFloat = 120
        static let fieldHeight: CGFloat = 32
    }

    enum Radius {
        static var control: CGFloat { ThemeStore.shape.control }
        static var card: CGFloat { ThemeStore.shape.card }
        static var tile: CGFloat { ThemeStore.shape.tile }
        static var surface: CGFloat { ThemeStore.shape.surface }
        static var bubble: CGFloat { ThemeStore.shape.bubble }
        static var composer: CGFloat { ThemeStore.shape.composer }
        static var artifact: CGFloat { ThemeStore.shape.artifact }
    }

    struct Shade {
        let opacity: Double
        let radius: CGFloat
        let y: CGFloat
    }

    enum Elevation {
        static let raised = Shade(opacity: 0.05, radius: 10, y: 6)
        static let lift = Shade(opacity: 0.10, radius: 16, y: 7)
        static let liftHover = Shade(opacity: 0.14, radius: 20, y: 9)
        static let floating = Shade(opacity: 0.18, radius: 24, y: 10)
        static let button = Shade(opacity: 0.22, radius: 12, y: 6)
        static let buttonHover = Shade(opacity: 0.30, radius: 18, y: 9)
        static let modal = Shade(opacity: 0.30, radius: 40, y: 18)
        static let sheet = Shade(opacity: 0.22, radius: 52, y: 22)
    }

    enum Rail {
        static let columnWidth: CGFloat = 248
        static let expandedWidth: CGFloat = 224
        static let collapsedWidth: CGFloat = 84
    }

    enum Window {
        static let mainMin = CGSize(width: 860, height: 520)
        static let aboutWidth: CGFloat = 300
    }

    enum Sheet {
        static let settings = CGSize(width: 920, height: 620)
        static let gallery = CGSize(width: 720, height: 560)
        static let modelDetailWidth: CGFloat = 620
        static let modelDetailHeight: CGFloat = 680
        static let modelRecipeHeight: CGFloat = 560
        static let promptWidth: CGFloat = 500
        static let promptHeight: CGFloat = 560
        static let serverWidth: CGFloat = 480
        static let serverHeight: CGFloat = 520
        static let installWidth: CGFloat = 760
        static let installHeight: CGFloat = 640
    }

    enum Column {
        static let settingsDetail: CGFloat = 640
        static let control: CGFloat = 220
        static let hero: CGFloat = 780
        static let prose: CGFloat = 520
        static let transcriptProse: CGFloat = 620
        static let emptyCaption: CGFloat = 380
        static let nowPlaying: CGFloat = 286
        static let nowPlayingLabel: CGFloat = 160
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
        static let artifactPlaceholder = CGSize(width: 340, height: 56)
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
                size: scaledSize(size, relativeTo: style), weight: weight, design: .default)
        }
        return .custom(family, size: size, relativeTo: style).weight(weight)
    }

    private static func uiStyled(
        _ style: Font.TextStyle, size: CGFloat, weight: Font.Weight = .regular
    ) -> Font {
        guard let family = fontBook.uiFamily else {
            return .system(
                size: scaledSize(size, relativeTo: style), weight: weight, design: .default)
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
    static let snap = Animation.spring(response: 0.25, dampingFraction: 0.9)
    static let press = Animation.easeOut(duration: 0.12)
    static let highlight = Animation.easeOut(duration: 0.4)

    static func motion(reduceMotion: Bool) -> Animation? {
        reduceMotion ? .easeOut(duration: 0.15) : spring
    }

    static func snapMotion(reduceMotion: Bool) -> Animation {
        reduceMotion ? .easeOut(duration: 0.15) : snap
    }

    static func reveal(_ shown: Bool, reduceMotion: Bool) -> Animation {
        reduceMotion ? .easeOut(duration: 0.15) : shown ? spring : wash
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
    static let dangerWash = danger.opacity(0.12)
    static let added = fixed(0x2EA043)

    enum PreviewPalette {
        static let lightPaper = fixed(ThemeFamily.standard.light.ground)
        static let lightSurface = fixed(ThemeFamily.standard.light.card)
        static let lightInk = fixed(ThemeFamily.standard.light.text)
        static let lightSoft = fixed(ThemeFamily.standard.light.muted)
        static let lightAccent = fixed(ThemeFamily.standard.light.accentDim)
        static let darkPaper = fixed(ThemeFamily.standard.dark.ground)
        static let darkSurface = fixed(ThemeFamily.standard.dark.card)
        static let darkInk = fixed(ThemeFamily.standard.dark.text)
        static let darkSoft = fixed(ThemeFamily.standard.dark.muted)
        static let darkAccent = fixed(ThemeFamily.standard.dark.accentDim)
    }

    static func fixed(_ hex: Int) -> Color {
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
                        ? ThemeStore.dark : ThemeStore.light
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

extension RoundedRectangle {
    static func soft(_ radius: CGFloat) -> RoundedRectangle {
        RoundedRectangle(cornerRadius: radius, style: .continuous)
    }
}

struct ModalScrim<Modal: View>: ViewModifier {
    let isPresented: Bool
    var anchor: UnitPoint = .center
    var handlesEscape = true
    let onDismiss: () -> Void
    @ViewBuilder let modal: () -> Modal
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .overlay {
                Group {
                    if isPresented {
                        Design.shadowColor.opacity(0.24)
                            .ignoresSafeArea()
                            .onTapGesture(perform: onDismiss)
                            .accessibilityLabel("Dismiss")
                            .transition(.opacity)
                    }
                }
                .animation(.easeOut(duration: 0.15), value: isPresented)
            }
            .overlay {
                Group {
                    if isPresented {
                        modal()
                            .background(
                                Design.paper,
                                in: RoundedRectangle.soft(Design.Radius.bubble))
                            .clipShape(RoundedRectangle.soft(Design.Radius.bubble))
                            .overlay(
                                RoundedRectangle.soft(Design.Radius.bubble)
                                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                            .shade(Design.Elevation.sheet)
                            .padding(Design.Space.xxl)
                            .onExitCommand(perform: handlesEscape ? onDismiss : nil)
                            .transition(
                                reduceMotion
                                    ? .opacity
                                    : .opacity.combined(
                                        with: .scale(scale: 0.96, anchor: anchor)))
                    }
                }
                .animation(
                    Design.reveal(isPresented, reduceMotion: reduceMotion),
                    value: isPresented)
            }
    }
}

extension View {
    func modalScrim<Modal: View>(
        isPresented: Bool, anchor: UnitPoint = .center, handlesEscape: Bool = true,
        onDismiss: @escaping () -> Void,
        @ViewBuilder modal: @escaping () -> Modal
    ) -> some View {
        modifier(
            ModalScrim(
                isPresented: isPresented, anchor: anchor, handlesEscape: handlesEscape,
                onDismiss: onDismiss, modal: modal))
    }
}

struct SurfaceCard: ViewModifier {
    var radius: CGFloat = Design.Radius.surface

    func body(content: Content) -> some View {
        content
            .background(Design.surface, in: RoundedRectangle.soft(radius))
            .overlay(
                RoundedRectangle.soft(radius)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(radius))
    }
}

struct Lifts: ViewModifier {
    let hovering: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .shade(hovering ? Design.Elevation.liftHover : Design.Elevation.lift)
            .offset(y: hovering && !reduceMotion ? -3 : 0)
            .animation(Design.wash, value: hovering)
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

    func hairlineBorder<S: InsettableShape>(_ shape: S, color: Color = Design.line) -> some View {
        overlay(shape.strokeBorder(color, lineWidth: Design.hairlineWidth))
    }

    func inkFocusRing<S: InsettableShape>(_ shape: S) -> some View {
        modifier(InkFocusRing(shape: shape))
    }
}

extension AnyTransition {
    static func arrive(from anchor: UnitPoint, reduceMotion: Bool = false) -> AnyTransition {
        reduceMotion
            ? .opacity
            : .opacity.combined(with: .scale(scale: 0.97, anchor: anchor))
    }
}

private struct DenyShake: GeometryEffect {
    var attempts: CGFloat
    var amplitude: CGFloat

    var animatableData: CGFloat {
        get { attempts }
        set { attempts = newValue }
    }

    func effectValue(size: CGSize) -> ProjectionTransform {
        ProjectionTransform(
            CGAffineTransform(
                translationX: amplitude * sin(attempts * .pi * 6), y: 0))
    }
}

struct DenyFeedback<S: InsettableShape>: ViewModifier {
    let attempts: Int
    let shape: S
    var amplitude: CGFloat = 6
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var flash = false

    func body(content: Content) -> some View {
        content
            .modifier(
                DenyShake(
                    attempts: CGFloat(attempts),
                    amplitude: reduceMotion ? 0 : amplitude))
            .animation(.easeOut(duration: 0.35), value: attempts)
            .overlay(
                shape.strokeBorder(
                    Design.heat.opacity(flash ? 0.55 : 0), lineWidth: Design.hairlineWidth))
            .onChange(of: attempts) {
                var immediate = Transaction()
                immediate.disablesAnimations = true
                withTransaction(immediate) { flash = true }
                withAnimation(.easeOut(duration: 0.25).delay(0.1)) { flash = false }
            }
    }
}

extension View {
    func denyShake<S: InsettableShape>(
        on attempts: Int, in shape: S, amplitude: CGFloat = 6
    ) -> some View {
        modifier(DenyFeedback(attempts: attempts, shape: shape, amplitude: amplitude))
    }
}

struct StaggeredArrival: ViewModifier {
    let index: Int
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var appeared = false

    func body(content: Content) -> some View {
        content
            .opacity(appeared ? 1 : 0)
            .offset(y: appeared || reduceMotion ? 0 : 3)
            .onAppear {
                guard !appeared else { return }
                if reduceMotion {
                    withAnimation(.easeOut(duration: 0.2)) { appeared = true }
                } else {
                    withAnimation(Design.spring.delay(0.04 * Double(min(index, 7)))) {
                        appeared = true
                    }
                }
            }
    }
}

extension View {
    func staggeredArrival(_ index: Int) -> some View {
        modifier(StaggeredArrival(index: index))
    }
}

struct InkButtonStyle: ButtonStyle {
    var circle = false
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme
    @State private var hovering = false

    private var shape: AnyInsettableShape {
        circle
            ? AnyInsettableShape(Circle())
            : AnyInsettableShape(RoundedRectangle.soft(Design.Radius.control))
    }

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Design.body.weight(.medium))
            .foregroundStyle(Design.paper)
            .padding(.horizontal, circle ? 0 : Design.Space.xl)
            .padding(.vertical, circle ? 0 : Design.Space.s + 1)
            .frame(width: circle ? 28 : nil, height: circle ? 28 : nil)
            .background(Design.ink, in: shape)
            .overlay(
                shape
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
            .contentShape(shape)
            .onHover { hovering = $0 }
            .inkFocusRing(shape)
            .animation(Design.wash, value: hovering)
            .animation(Design.press, value: configuration.isPressed)
    }
}

struct FilterChip: View {
    let label: String
    let isOn: Bool
    var mark: SourceKind? = nil
    var count: Int? = nil
    var isDisabled = false
    let action: () -> Void

    init(
        label: String, isOn: Bool, mark: SourceKind? = nil, count: Int? = nil,
        isDisabled: Bool = false, action: @escaping () -> Void
    ) {
        self.label = label
        self.isOn = isOn
        self.mark = mark
        self.count = count
        self.isDisabled = isDisabled
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
                if let count {
                    Text("\(count)")
                        .font(Design.micro)
                        .monospacedDigit()
                        .foregroundStyle(isOn ? Design.paper.opacity(0.6) : Design.inkFaint)
                }
            }
            .foregroundStyle(isOn ? Design.paper : hovering ? Design.ink : Design.inkSoft)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .background(
                isOn
                    ? AnyShapeStyle(Design.ink)
                    : hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.surface),
                in: RoundedRectangle.soft(Design.Radius.control))
            .overlay(
                RoundedRectangle.soft(Design.Radius.control)
                    .strokeBorder(
                        isOn ? AnyShapeStyle(.clear) : Design.hairline,
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
            .opacity(isDisabled ? 0.45 : 1)
        }
        .buttonStyle(PressDipStyle())
        .disabled(isDisabled)
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle.soft(Design.Radius.control))
        .animation(Design.wash, value: hovering)
        .animation(Design.wash, value: isOn)
        .accessibilityLabel(count.map { "\(label), \($0)" } ?? label)
        .accessibilityAddTraits(isOn ? .isSelected : [])
    }
}

struct ChipDivider: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 1)
            .fill(Design.line)
            .frame(width: 2, height: 16)
            .accessibilityHidden(true)
    }
}

struct PressDipStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.72 : 1)
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(Design.press, value: configuration.isPressed)
    }
}

struct ConfirmingButton: View {
    enum Appearance { case tray, micro, plain }

    let label: String
    let confirmedLabel: String
    var glyph = "doc.on.doc"
    var confirmedGlyph = "checkmark"
    var appearance: Appearance = .tray
    var holdFor: Duration = .seconds(2)
    let attempt: () async -> Bool
    @State private var confirmed = false
    @State private var revert: Task<Void, Never>?
    @State private var hovering = false

    init(
        label: String, confirmedLabel: String, glyph: String = "doc.on.doc",
        confirmedGlyph: String = "checkmark", appearance: Appearance = .tray,
        holdFor: Duration = .seconds(2), action: @escaping () -> Void
    ) {
        self.label = label
        self.confirmedLabel = confirmedLabel
        self.glyph = glyph
        self.confirmedGlyph = confirmedGlyph
        self.appearance = appearance
        self.holdFor = holdFor
        self.attempt = {
            action()
            return true
        }
    }

    init(
        label: String, confirmedLabel: String, glyph: String = "doc.on.doc",
        confirmedGlyph: String = "checkmark", appearance: Appearance = .tray,
        holdFor: Duration = .seconds(2), attempt: @escaping () async -> Bool
    ) {
        self.label = label
        self.confirmedLabel = confirmedLabel
        self.glyph = glyph
        self.confirmedGlyph = confirmedGlyph
        self.appearance = appearance
        self.holdFor = holdFor
        self.attempt = attempt
    }

    var body: some View {
        Group {
            switch appearance {
            case .plain:
                button.buttonStyle(QuietButtonStyle())
            case .tray, .micro:
                button.buttonStyle(PressDipStyle())
            }
        }
        .onHover { hovering = $0 }
        .onDisappear { revert?.cancel() }
        .animation(Design.wash, value: hovering)
        .animation(Design.snap, value: confirmed)
        .accessibilityLabel(confirmed ? confirmedLabel : label)
    }

    private var button: some View {
        Button(action: fire) { content }
    }

    private func fire() {
        revert?.cancel()
        revert = Task {
            let succeeded = await attempt()
            guard !Task.isCancelled else { return }
            confirmed = succeeded
            guard succeeded else { return }
            try? await Task.sleep(for: holdFor)
            guard !Task.isCancelled else { return }
            confirmed = false
        }
    }

    @ViewBuilder
    private var content: some View {
        switch appearance {
        case .tray:
            HStack(spacing: Design.Space.xs) {
                Image(systemName: confirmed ? confirmedGlyph : glyph)
                    .font(Design.glyphSmall)
                    .contentTransition(.symbolEffect(.replace))
                    .symbolEffect(.bounce, value: confirmed)
                Text(confirmed ? confirmedLabel : label)
                    .font(Design.caption.weight(.medium))
                    .lineLimit(1)
                    .fixedSize()
                    .contentTransition(.opacity)
            }
            .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
            .padding(.horizontal, Design.Space.s)
            .padding(.vertical, Design.Space.xxs + 1)
            .contentShape(Rectangle())
        case .micro:
            Text(confirmed ? confirmedLabel : label)
                .font(Design.micro)
                .tracking(Design.microTracking)
                .contentTransition(.opacity)
                .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                .contentShape(Rectangle())
        case .plain:
            Text(confirmed ? confirmedLabel : label)
                .contentTransition(.opacity)
        }
    }
}

struct MicroHeader: View {
    let title: String

    var body: some View {
        Text(title)
            .font(Design.label.weight(.semibold))
            .foregroundStyle(Design.inkFaint)
    }
}

struct GlyphPlaque: View {
    let glyph: String

    var body: some View {
        Image(systemName: glyph)
            .font(.system(size: 22, weight: .regular))
            .foregroundStyle(Design.inkSoft)
            .frame(width: 60, height: 60)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .shade(Design.Elevation.raised)
    }
}

struct ConfirmableIconButton: View {
    var glyph = "xmark.circle.fill"
    let label: String
    let confirmLabel: String
    let action: () -> Void
    @State private var armed = false
    @State private var disarm: Task<Void, Never>?
    @State private var hovering = false

    var body: some View {
        Button {
            if armed {
                disarm?.cancel()
                armed = false
                action()
            } else {
                armed = true
                disarm?.cancel()
                disarm = Task {
                    try? await Task.sleep(for: .seconds(4))
                    guard !Task.isCancelled else { return }
                    armed = false
                }
            }
        } label: {
            Group {
                if armed {
                    Text(confirmLabel)
                        .font(Design.label.weight(.semibold))
                        .foregroundStyle(Design.heatText)
                } else {
                    Image(systemName: glyph)
                        .font(Design.glyphInline)
                        .foregroundStyle(hovering ? Design.ink : Design.inkFaint)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(PressDipStyle())
        .onHover { inside in
            hovering = inside
            if !inside && armed {
                armed = false
                disarm?.cancel()
            }
        }
        .onDisappear { disarm?.cancel() }
        .onExitCommand {
            armed = false
            disarm?.cancel()
        }
        .animation(Design.snap, value: armed)
        .animation(Design.wash, value: hovering)
        .accessibilityLabel(armed ? confirmLabel : label)
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
            Text(text)
                .font(Design.label.weight(.medium))
                .lineLimit(1)
        }
        .foregroundStyle(
            live ? Design.accentText : faint ? Design.inkFaint : Design.inkSoft
        )
        .padding(.horizontal, Design.Space.m)
        .padding(.vertical, Design.Space.xxs + 1.5)
        .background(
            live ? AnyShapeStyle(Design.accentWash) : AnyShapeStyle(Design.inkWash),
            in: RoundedRectangle.soft(Design.Radius.control))
        .overlay(
            RoundedRectangle.soft(Design.Radius.control).strokeBorder(
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
            .background(Design.cardFill, in: RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
    }
}

struct ClampedSheetFrame: ViewModifier {
    let width: CGFloat
    let height: CGFloat

    func body(content: Content) -> some View {
        GeometryReader { geo in
            content.frame(width: geo.size.width, height: geo.size.height)
        }
        .frame(maxWidth: width, maxHeight: height)
    }
}

extension View {
    func clampedSheetFrame(width: CGFloat, height: CGFloat) -> some View {
        modifier(ClampedSheetFrame(width: width, height: height))
    }
}

struct SheetDivider: View {
    var body: some View {
        Rectangle()
            .fill(Design.hairline)
            .frame(height: Design.hairlineWidth)
            .accessibilityHidden(true)
    }
}

struct RowRule: View {
    var body: some View {
        Rectangle()
            .fill(Design.line)
            .frame(height: Design.hairlineWidth)
            .accessibilityHidden(true)
    }
}

struct SettingsGroup<Content: View>: View {
    let header: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: header)
            VStack(alignment: .leading, spacing: 0) {
                content()
            }
            .padding(.horizontal, Design.Space.xl)
            .padding(.vertical, Design.Space.s)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }
}

struct SheetHeader<Plaque: View, Below: View>: View {
    let title: String
    var subtitle: String? = nil
    let onClose: () -> Void
    @ViewBuilder var plaque: () -> Plaque
    @ViewBuilder var below: () -> Below

    var body: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            IconPlaque(size: 44, content: plaque)
            VStack(alignment: .leading, spacing: Design.Space.s) {
                Text(title)
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                }
                below()
            }
            Spacer(minLength: Design.Space.l)
            SheetCloseButton(action: onClose)
        }
        .padding(.horizontal, Design.Space.gutter)
        .padding(.top, Design.Space.gutter)
        .padding(.bottom, Design.Space.xl)
    }
}

extension SheetHeader where Below == EmptyView {
    init(
        title: String, subtitle: String? = nil, onClose: @escaping () -> Void,
        @ViewBuilder plaque: @escaping () -> Plaque
    ) {
        self.init(
            title: title, subtitle: subtitle, onClose: onClose, plaque: plaque,
            below: { EmptyView() })
    }
}

extension View {
    func sheetBodyPadding() -> some View {
        padding(.horizontal, Design.Space.gutter).padding(.vertical, Design.Space.xl)
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
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(hovering ? Design.inkWash : .clear))
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle.soft(Design.Radius.control))
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
                Circle()
                    .stroke(Design.heat, lineWidth: Design.hairlineWidth)
                    .scaleEffect(pulsing ? 2.4 : 1)
                    .opacity(pulsing ? 0 : 0.8)
            }
            Circle()
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

struct ScanningTag: View {
    let active: Bool
    var uppercased = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var label: String { uppercased ? "Scanning…".uppercased() : "Scanning…" }

    var body: some View {
        Text(label)
            .font(Design.micro)
            .tracking(uppercased ? Design.microTracking : 0)
            .lineLimit(1)
            .hidden()
            .overlay {
                if active {
                    ShimmerText(text: label, tracked: uppercased)
                        .transition(.opacity)
                }
            }
            .animation(Design.motion(reduceMotion: reduceMotion), value: active)
    }
}

struct SkeletonPulse: View {
    var radius: CGFloat = Design.Radius.card
    @State private var bright = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        RoundedRectangle.soft(radius)
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
    var usesCancelShortcut = true
    var diameter: CGFloat = 24
    var glyph: Font = Design.glyphSmall.weight(.bold)
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
                .font(glyph)
                .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
                .frame(width: diameter, height: diameter)
                .background(
                    hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.cardFill),
                    in: Circle())
                .contentShape(Circle())
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .inkFocusRing(Circle())
        .keyboardShortcut(usesCancelShortcut ? .cancelAction : nil)
        .accessibilityLabel("Close")
    }
}

struct QuietDisclosureStyle: DisclosureGroupStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Button {
                withAnimation(Design.motion(reduceMotion: reduceMotion)) {
                    configuration.isExpanded.toggle()
                }
            } label: {
                HStack(spacing: Design.Space.xs) {
                    configuration.label
                    Image(systemName: "chevron.right")
                        .font(Design.glyphMicro)
                        .foregroundStyle(Design.inkFaint)
                        .rotationEffect(.degrees(configuration.isExpanded ? 90 : 0))
                }
            }
            .buttonStyle(.plain)
            if configuration.isExpanded {
                configuration.content
                    .transition(.opacity)
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
            let url = Bundle.appModule.url(forResource: "Resources/logo", withExtension: "svg")
                ?? Bundle.appModule.url(forResource: "logo", withExtension: "svg"),
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

struct TypingDots: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var bounce = false

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { index in
                Circle()
                    .fill(Design.inkSoft)
                    .frame(width: 5, height: 5)
                    .opacity(bounce ? 1 : 0.3)
                    .animation(
                        reduceMotion
                            ? nil
                            : .easeInOut(duration: 0.5)
                                .repeatForever(autoreverses: true)
                                .delay(Double(index) * 0.16),
                        value: bounce)
            }
        }
        .frame(height: 16)
        .onAppear { bounce = true }
        .accessibilityLabel("Working")
    }
}

struct SpeakingIndicator: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var phase = false

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { index in
                Capsule()
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

enum Haptics {
    @MainActor
    static func completion() {
        NSHapticFeedbackManager.defaultPerformer.perform(
            .levelChange, performanceTime: .drawCompleted)
    }
}

extension Int {
    var compactCount: String {
        switch self {
        case 1_000_000...:
            String(format: "%.1fM", Double(self) / 1_000_000)
        case 1_000...:
            String(format: "%.1fk", Double(self) / 1_000)
        default:
            String(self)
        }
    }
}
