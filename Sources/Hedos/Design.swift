import HedosKernel
import SwiftUI

enum Design {
    static let accent = Color(red: 0.83, green: 0.66, blue: 0.34)
    static let warn = Color(red: 0.86, green: 0.60, blue: 0.38)

    static func modalityColor(_ modality: Modality) -> Color {
        switch modality {
        case .text: Color(red: 0.42, green: 0.58, blue: 0.90)
        case .speech, .audio: Color(red: 0.36, green: 0.72, blue: 0.51)
        case .image: Color(red: 0.86, green: 0.60, blue: 0.38)
        default: Color(white: 0.55)
        }
    }

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
        case .library: "Library"
        case .settings: "Settings"
        }
    }

    static func plaque(_ size: CGFloat, weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func data(_ size: CGFloat) -> Font {
        .system(size: size, weight: .regular, design: .monospaced)
    }
}

extension View {
    func hedosField() -> some View {
        padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 12))
            .overlay(
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(.quaternary, lineWidth: 1))
    }
}

struct HeptagonMark: View {
    var size: CGFloat
    var color: Color = .primary

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
    @State private var phase = false

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { index in
                Capsule()
                    .fill(.secondary)
                    .frame(width: 2.5, height: phase ? barHeight(index) : 4)
                    .animation(
                        .easeInOut(duration: 0.45)
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
