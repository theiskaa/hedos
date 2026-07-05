import AppKit
import CoreGraphics
import Foundation
import UniformTypeIdentifiers

let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()

func drawIcon(size: Int) -> CGImage {
    let s = CGFloat(size)
    let context = CGContext(
        data: nil,
        width: size,
        height: size,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

    let margin = s * 100 / 1024
    let tile = CGRect(x: margin, y: margin, width: s - 2 * margin, height: s - 2 * margin)
    let radius = s * 185 / 1024

    context.saveGState()
    context.addPath(
        CGPath(roundedRect: tile, cornerWidth: radius, cornerHeight: radius, transform: nil))
    context.clip()
    let colors =
        [
            CGColor(red: 1, green: 1, blue: 1, alpha: 1),
            CGColor(red: 0.94, green: 0.94, blue: 0.95, alpha: 1),
        ] as CFArray
    let gradient = CGGradient(
        colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!, colors: colors, locations: [0, 1])!
    context.drawLinearGradient(
        gradient,
        start: CGPoint(x: s / 2, y: tile.maxY),
        end: CGPoint(x: s / 2, y: tile.minY),
        options: [])
    context.restoreGState()

    let center = CGPoint(x: s / 2, y: s / 2)
    let heptRadius = tile.width * 0.345
    let path = CGMutablePath()
    for index in 0..<7 {
        let angle = (Double(index) * 2 * .pi / 7) + .pi / 2
        let point = CGPoint(
            x: center.x + heptRadius * cos(angle),
            y: center.y + heptRadius * sin(angle))
        if index == 0 { path.move(to: point) } else { path.addLine(to: point) }
    }
    path.closeSubpath()

    let ink = CGColor(red: 0.04, green: 0.04, blue: 0.05, alpha: 1)
    context.setFillColor(ink)
    context.setStrokeColor(ink)
    context.setLineJoin(.round)
    context.setLineWidth(tile.width * 0.07)
    context.addPath(path)
    context.drawPath(using: .fillStroke)

    return context.makeImage()!
}

func writePNG(_ image: CGImage, to url: URL) {
    let destination = CGImageDestinationCreateWithURL(
        url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, image, nil)
    CGImageDestinationFinalize(destination)
}

let iconset = FileManager.default.temporaryDirectory
    .appendingPathComponent("Hedos-\(ProcessInfo.processInfo.processIdentifier).iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

let specs: [(Int, String)] = [
    (16, "icon_16x16"), (32, "icon_16x16@2x"),
    (32, "icon_32x32"), (64, "icon_32x32@2x"),
    (128, "icon_128x128"), (256, "icon_128x128@2x"),
    (256, "icon_256x256"), (512, "icon_256x256@2x"),
    (512, "icon_512x512"), (1024, "icon_512x512@2x"),
]
for (size, name) in specs {
    writePNG(drawIcon(size: size), to: iconset.appendingPathComponent("\(name).png"))
}

let output = repoRoot.appendingPathComponent("Sources/Hedos/Resources/Hedos.icns")
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
process.arguments = ["-c", "icns", iconset.path, "-o", output.path]
try! process.run()
process.waitUntilExit()
try? FileManager.default.removeItem(at: iconset)
print(process.terminationStatus == 0 ? "wrote \(output.path)" : "iconutil failed")
exit(process.terminationStatus)
