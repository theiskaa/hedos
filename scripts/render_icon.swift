import AppKit
import CoreGraphics
import Foundation
import UniformTypeIdentifiers

let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()

let artworkURL = repoRoot.appendingPathComponent(
    "Icon Exports/Icon-iOS-Dark-1024x1024@4x.png")
let artworkSource = CGImageSourceCreateWithURL(artworkURL as CFURL, nil)!
let artwork = CGImageSourceCreateImageAtIndex(artworkSource, 0, nil)!

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

    context.interpolationQuality = .high
    context.draw(artwork, in: tile)

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
