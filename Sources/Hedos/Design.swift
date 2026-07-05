import HedosKernel
import SwiftUI

enum Design {
    static let accent = Color(red: 0.83, green: 0.66, blue: 0.34)
    static let lapis = Color(red: 0.42, green: 0.58, blue: 0.90)
    static let laurel = Color(red: 0.36, green: 0.72, blue: 0.51)
    static let terracotta = Color(red: 0.86, green: 0.60, blue: 0.38)
    static let granite = Color(white: 0.55)

    static func modalityColor(_ modality: Modality) -> Color {
        switch modality {
        case .text: lapis
        case .speech, .audio: laurel
        case .image: terracotta
        default: granite
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

    static func plaque(_ size: CGFloat, weight: Font.Weight = .medium) -> Font {
        .system(size: size, weight: weight, design: .serif)
    }

    static func data(_ size: CGFloat) -> Font {
        .system(size: size, weight: .regular, design: .monospaced)
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
                    .fill(Design.laurel)
                    .frame(width: 3, height: phase ? barHeight(index) : 4)
                    .animation(
                        .easeInOut(duration: 0.45)
                            .repeatForever(autoreverses: true)
                            .delay(Double(index) * 0.09),
                        value: phase)
            }
        }
        .frame(height: 18)
        .onAppear { phase = true }
        .accessibilityLabel("Speaking")
    }

    private func barHeight(_ index: Int) -> CGFloat {
        [10, 16, 18, 14, 9][index]
    }
}
