import AppKit
import HedosKernel
import SwiftUI

struct SourceMark: View {
    let kind: SourceKind
    var size: CGFloat = 20

    var body: some View {
        Group {
            if let brand = Self.brandImage(for: kind) {
                Image(nsImage: brand.image)
                    .renderingMode(brand.template ? .template : .original)
                    .resizable()
                    .scaledToFit()
            } else {
                Image(systemName: fallbackGlyph)
                    .font(.system(size: size * 0.85, weight: .medium))
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }

    private var fallbackGlyph: String {
        switch kind {
        case .file: "doc"
        case .folder: "folder"
        case .builtin: "apple.logo"
        case .endpoint: "network"
        default: "shippingbox"
        }
    }

    struct Brand {
        let image: NSImage
        let template: Bool
    }

    private static let cache: [String: Brand] = {
        var brands: [String: Brand] = [:]
        let manifest: [(slug: String, ext: String, template: Bool)] = [
            ("ollama", "svg", true),
            ("huggingface", "svg", false),
            ("lmstudio", "png", false),
        ]
        for entry in manifest {
            guard
                let url = Bundle.appModule.url(
                    forResource: "Resources/Brands/\(entry.slug)", withExtension: entry.ext)
                    ?? Bundle.appModule.url(forResource: entry.slug, withExtension: entry.ext),
                let image = NSImage(contentsOf: url)
            else { continue }
            image.isTemplate = entry.template
            brands[entry.slug] = Brand(image: image, template: entry.template)
        }
        return brands
    }()

    private static func brandImage(for kind: SourceKind) -> Brand? {
        switch kind {
        case .ollama: cache["ollama"]
        case .huggingfaceCache: cache["huggingface"]
        case .lmStudio: cache["lmstudio"]
        default: nil
        }
    }
}
