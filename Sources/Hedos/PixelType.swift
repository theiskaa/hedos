import AppKit
import SwiftUI

struct HedosLogo: View {
    var size: CGFloat
    var color: Color = Design.ink

    private static let artwork: NSImage? = {
        guard
            let url = Bundle.module.url(
                forResource: "hedos", withExtension: "svg", subdirectory: "Resources")
                ?? Bundle.module.url(forResource: "hedos", withExtension: "svg"),
            let image = NSImage(contentsOf: url)
        else { return nil }
        image.isTemplate = true
        return image
    }()

    var body: some View {
        if let artwork = Self.artwork {
            Image(nsImage: artwork)
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .foregroundStyle(color)
                .frame(width: size, height: size)
                .accessibilityHidden(true)
        }
    }
}

struct PixelKoala: View {
    var size: CGFloat
    var color: Color = Design.ink

    var body: some View {
        Canvas { context, canvasSize in
            let rows = Self.bitmap.count
            let cols = Self.bitmap[0].count
            let cell = min(canvasSize.width / CGFloat(cols), canvasSize.height / CGFloat(rows))
            let originX = (canvasSize.width - cell * CGFloat(cols)) / 2
            let originY = (canvasSize.height - cell * CGFloat(rows)) / 2
            for (y, row) in Self.bitmap.enumerated() {
                for (x, mark) in row.enumerated() where mark == "1" {
                    let rect = CGRect(
                        x: originX + CGFloat(x) * cell,
                        y: originY + CGFloat(y) * cell,
                        width: cell, height: cell)
                    context.fill(Path(rect), with: .color(color))
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    static let bitmap: [[Character]] = [
        "0000001111110000000000011111110000",
        "0000111111111000111100111111111000",
        "0000111100111111111111111100111000",
        "0000110011011111111111110011011100",
        "0001110111111111111111111111101100",
        "0001110111111111111111111111001100",
        "0001110111111111111111111111011100",
        "0001110111111111111111111111011100",
        "0000111011111111111111111111011100",
        "0000111111110011111110011111111000",
        "0000011111110011000110011111110000",
        "0000000111111110111011111111100000",
        "0000000001111110111011111110000000",
        "0000000001111110111011111110000000",
        "0000000111111110010011111100001111",
        "0000001000100000111000000100001001",
        "0000010000110000000000001000010010",
        "0000100000011000000000010000010010",
        "0001000000000111000011100000010010",
        "0010000000000001111111000001100100",
        "0100000000000000000101100001100101",
        "0100000000000000000011111001100111",
        "1000000000000000000011001111000011",
        "1000000000000000000001000001000010",
        "1000000000000000000000100001001100",
        "0000000011111110000000011111101100",
        "0000001100000011000000000100111000",
        "0000001000000000100000000000111000",
        "0000000000000000110000000000001100",
        "0000000000000000011111100000000100",
        "0000000000000000010100011111111000",
        "0000000000000000010100000100110000",
        "0000000000000000001100000100100000",
        "0000000000000000001000001001100000",
        "1000000000000000001111001001000000",
    ].map(Array.init)
}

struct HedosWordmark: View {
    var unit: CGFloat = 4
    var color: Color = Design.ink
    private let text = "hedos"

    var body: some View {
        Canvas { context, _ in
            var cursor: CGFloat = 0
            for character in text {
                if let glyph = Self.glyphs[character] {
                    for (y, row) in glyph.enumerated() {
                        for (x, mark) in row.enumerated() where mark == "1" {
                            let rect = CGRect(
                                x: cursor + CGFloat(x) * unit,
                                y: CGFloat(y) * unit,
                                width: unit, height: unit)
                            context.fill(Path(rect), with: .color(color))
                        }
                    }
                }
                cursor += unit * 6
            }
        }
        .frame(width: unit * CGFloat(text.count) * 6, height: unit * 7)
        .accessibilityLabel("hedos")
    }

    static let glyphs: [Character: [[Character]]] = [
        "h": ["10000", "10000", "10000", "11110", "10010", "10010", "10010"].map(Array.init),
        "e": ["00000", "00000", "01100", "10010", "11110", "10000", "01110"].map(Array.init),
        "d": ["00010", "00010", "00010", "01110", "10010", "10010", "01110"].map(Array.init),
        "o": ["00000", "00000", "01100", "10010", "10010", "10010", "01100"].map(Array.init),
        "s": ["00000", "00000", "01110", "10000", "01100", "00010", "11100"].map(Array.init),
    ]
}

struct PixelNumber: View {
    let text: String
    var unit: CGFloat = 5
    var color: Color = Design.ink

    var body: some View {
        Canvas { context, _ in
            var cursor: CGFloat = 0
            for character in text {
                if let glyph = Self.digits[character] {
                    for (y, row) in glyph.enumerated() {
                        for (x, mark) in row.enumerated() where mark == "1" {
                            let rect = CGRect(
                                x: cursor + CGFloat(x) * unit,
                                y: CGFloat(y) * unit,
                                width: unit, height: unit)
                            context.fill(Path(rect), with: .color(color))
                        }
                    }
                    cursor += unit * 6
                } else {
                    cursor += unit * 3
                }
            }
        }
        .frame(width: metrics.width, height: unit * 7)
        .accessibilityLabel(text)
    }

    private var metrics: (width: CGFloat, height: CGFloat) {
        var width: CGFloat = 0
        for character in text {
            width += Self.digits[character] != nil ? unit * 6 : unit * 3
        }
        return (max(0, width - unit), unit * 7)
    }

    static let digits: [Character: [[Character]]] = [
        "0": ["01110", "10001", "10011", "10101", "11001", "10001", "01110"].map(Array.init),
        "1": ["00100", "01100", "00100", "00100", "00100", "00100", "01110"].map(Array.init),
        "2": ["01110", "10001", "00001", "00110", "01000", "10000", "11111"].map(Array.init),
        "3": ["11111", "00010", "00100", "00010", "00001", "10001", "01110"].map(Array.init),
        "4": ["00010", "00110", "01010", "10010", "11111", "00010", "00010"].map(Array.init),
        "5": ["11111", "10000", "11110", "00001", "00001", "10001", "01110"].map(Array.init),
        "6": ["00110", "01000", "10000", "11110", "10001", "10001", "01110"].map(Array.init),
        "7": ["11111", "00001", "00010", "00100", "01000", "01000", "01000"].map(Array.init),
        "8": ["01110", "10001", "10001", "01110", "10001", "10001", "01110"].map(Array.init),
        "9": ["01110", "10001", "10001", "01111", "00001", "00010", "01100"].map(Array.init),
    ]
}

struct SegmentedBar: View {
    let used: Double
    let warm: Double
    var segments: Int = 20

    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<segments, id: \.self) { index in
                let threshold = Double(index + 1) / Double(segments)
                RoundedRectangle(cornerRadius: 1)
                    .fill(fill(threshold))
                    .frame(height: 14)
            }
        }
        .accessibilityHidden(true)
    }

    private func fill(_ threshold: Double) -> Color {
        if threshold <= warm { return Design.heat }
        if threshold <= used { return Design.accentText }
        return Design.line
    }
}

struct DottedRule: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            path.move(to: CGPoint(x: 0, y: size.height / 2))
            path.addLine(to: CGPoint(x: size.width, y: size.height / 2))
            context.stroke(
                path, with: .color(Design.line),
                style: StrokeStyle(lineWidth: 1, dash: [1.5, 3]))
        }
        .frame(height: 1)
        .allowsHitTesting(false)
    }
}

struct PixelGrid: View {
    var step: CGFloat = 16
    var opacity: Double = 0.05

    var body: some View {
        Canvas { context, size in
            var path = Path()
            var x: CGFloat = 0
            while x <= size.width {
                path.move(to: CGPoint(x: x, y: 0))
                path.addLine(to: CGPoint(x: x, y: size.height))
                x += step
            }
            var y: CGFloat = 0
            while y <= size.height {
                path.move(to: CGPoint(x: 0, y: y))
                path.addLine(to: CGPoint(x: size.width, y: y))
                y += step
            }
            context.stroke(path, with: .color(Design.ink.opacity(opacity)), lineWidth: 0.5)
        }
        .allowsHitTesting(false)
    }
}
