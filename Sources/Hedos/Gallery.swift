import AppKit
import HedosKernel
import ImageIO
import SwiftUI

@Observable
@MainActor
final class GalleryModel {
    private let kernel: Kernel
    private var thumbnails: [String: NSImage] = [:]

    var artifacts: [Artifact] = []
    var notice: String?

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    var arranged: [Artifact] {
        Gallery.arrange(artifacts, modelID: nil, sort: .newestFirst)
    }

    func artifact(id: String?) -> Artifact? {
        guard let id else { return nil }
        return artifacts.first { $0.id == id }
    }

    func load() async {
        let all = (try? await kernel.artifactStore.list()) ?? []
        artifacts = all.filter { $0.capability == .image }
    }

    func thumbnail(_ artifact: Artifact) -> NSImage? {
        thumbnails[artifact.id]
    }

    func loadThumbnail(_ artifact: Artifact) async {
        guard thumbnails[artifact.id] == nil else { return }
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let maxPixel = Design.Bubble.imageMax * scale
        if let url = try? await kernel.artifactStore.url(id: artifact.id) {
            let sharp = await Task.detached(priority: .utility) {
                Self.downsampled(url, maxPixel: maxPixel)
            }.value
            if let sharp {
                thumbnails[artifact.id] = sharp
                return
            }
        }
        guard let data = try? await kernel.artifactStore.previewData(id: artifact.id) else {
            return
        }
        let fallback = await Task.detached(priority: .utility) { NSImage(data: data) }.value
        if let fallback {
            thumbnails[artifact.id] = fallback
        }
    }

    func fullImage(_ artifact: Artifact) async -> NSImage? {
        if let url = try? await kernel.artifactStore.url(id: artifact.id) {
            let full = await Task.detached(priority: .userInitiated) {
                NSImage(contentsOf: url)
            }.value
            if let full { return full }
        }
        guard let data = try? await kernel.artifactStore.previewData(id: artifact.id) else {
            return thumbnails[artifact.id]
        }
        let fallback = await Task.detached(priority: .userInitiated) { NSImage(data: data) }.value
        return fallback ?? thumbnails[artifact.id]
    }

    func delete(_ artifact: Artifact) async {
        do {
            try await kernel.artifactStore.delete(id: artifact.id)
            thumbnails[artifact.id] = nil
            await load()
        } catch {
            notice = error.localizedDescription
        }
    }

    func download(_ artifact: Artifact) {
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactStore.url(id: artifact.id) else { return }
            let panel = NSSavePanel()
            panel.nameFieldStringValue = url.lastPathComponent
            panel.begin { response in
                guard response == .OK, let destination = panel.url else { return }
                do {
                    try AtomicFileWrite.copy(from: url, to: destination)
                } catch {
                    self.notice = error.localizedDescription
                }
            }
        }
    }

    nonisolated static func downsampled(_ url: URL, maxPixel: CGFloat) -> NSImage? {
        let sourceOptions = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithURL(url as CFURL, sourceOptions) else {
            return nil
        }
        let thumbnailOptions =
            [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: maxPixel,
            ] as [CFString: Any] as CFDictionary
        guard let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, thumbnailOptions)
        else { return nil }
        return NSImage(cgImage: cgImage, size: NSSize(width: cgImage.width, height: cgImage.height))
    }
}
