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

    func copy(_ artifact: Artifact) {
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactStore.url(id: artifact.id),
                await ImagePasteboard.copy(fileURL: url)
            else {
                notice = "Couldn't copy the image."
                return
            }
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

struct GallerySheet: View {
    @Bindable var shell: ShellModel
    @Binding var viewing: Artifact?
    @Binding var deleting: Artifact?
    let onClose: () -> Void
    @State private var hoveredCell: String?

    private let columns = [GridItem(.adaptive(minimum: 168), spacing: Design.Space.tile)]

    var body: some View {
        VStack(spacing: 0) {
            SheetHeader(
                title: "Gallery",
                subtitle: headerSubtitle,
                onClose: onClose,
                plaque: {
                    Image(systemName: "photo.stack")
                        .font(Design.glyphPrimary)
                        .foregroundStyle(Design.inkSoft)
                })
            if shell.gallery.arranged.isEmpty {
                emptyState
            } else {
                grid
            }
        }
        .clampedSheetFrame(
            width: Design.Sheet.gallery.width, height: Design.Sheet.gallery.height)
        .task {
            await shell.gallery.load()
            if let id = shell.galleryFocusID, let artifact = shell.gallery.artifact(id: id) {
                viewing = artifact
            }
            shell.galleryFocusID = nil
        }
    }

    private var grid: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Design.Space.pane) {
                ForEach(sections) { section in
                    VStack(alignment: .leading, spacing: Design.Space.l) {
                        HStack(spacing: Design.Space.m) {
                            MicroHeader(title: section.title)
                            Text("\(section.items.count)")
                                .font(Design.label.weight(.medium))
                                .foregroundStyle(Design.inkFaint)
                            Spacer(minLength: 0)
                        }
                        LazyVGrid(columns: columns, spacing: Design.Space.tile) {
                            ForEach(section.items) { artifact in
                                cell(artifact)
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xs)
            .padding(.bottom, Design.Space.gutter)
        }
    }

    private var countLine: String {
        let count = shell.gallery.arranged.count
        return count == 1 ? "1 image" : "\(count) images"
    }

    private var headerSubtitle: String {
        guard !shell.gallery.arranged.isEmpty else { return "Everything you've made, here" }
        return "\(countLine) · everything you've made"
    }

    private struct Section: Identifiable {
        let id: String
        let title: String
        let items: [Artifact]
    }

    private var sections: [Section] {
        let calendar = Calendar.current
        let startNow = calendar.startOfDay(for: Date())
        var today: [Artifact] = []
        var week: [Artifact] = []
        var earlier: [Artifact] = []
        for artifact in shell.gallery.arranged {
            let start = calendar.startOfDay(for: artifact.createdAt)
            let days = calendar.dateComponents([.day], from: start, to: startNow).day ?? 0
            if days <= 0 {
                today.append(artifact)
            } else if days < 7 {
                week.append(artifact)
            } else {
                earlier.append(artifact)
            }
        }
        var result: [Section] = []
        if !today.isEmpty { result.append(Section(id: "today", title: "Today", items: today)) }
        if !week.isEmpty { result.append(Section(id: "week", title: "This Week", items: week)) }
        if !earlier.isEmpty {
            result.append(Section(id: "earlier", title: "Earlier", items: earlier))
        }
        return result
    }

    private var emptyState: some View {
        VStack(spacing: Design.Space.l) {
            IconPlaque(size: 56) {
                Image(systemName: "photo.on.rectangle.angled")
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkFaint)
            }
            Text("No images yet")
                .font(Design.title)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
            Text("Everything you generate lands here, ready to save or revisit.")
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(2.5)
                .multilineTextAlignment(.center)
                .frame(maxWidth: Design.Column.emptyCaption)
        }
        .padding(Design.Space.pane)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func cell(_ artifact: Artifact) -> some View {
        let hovered = hoveredCell == artifact.id
        return Button {
            viewing = artifact
        } label: {
            VStack(alignment: .leading, spacing: Design.Space.m) {
                thumbnail(artifact)
                caption(artifact)
            }
            .padding(Design.Space.m)
            .frame(maxWidth: .infinity)
            .tile(hovering: hovered)
        }
        .buttonStyle(PressDipStyle())
        .overlay(alignment: .topTrailing) {
            if hovered {
                actions(artifact)
                    .padding(Design.Space.m + Design.Space.xxs)
                    .transition(.opacity)
            }
        }
        .onHover { inside in
            if inside {
                hoveredCell = artifact.id
            } else if hoveredCell == artifact.id {
                hoveredCell = nil
            }
        }
        .animation(Design.wash, value: hoveredCell)
        .task(id: artifact.id) {
            await shell.gallery.loadThumbnail(artifact)
        }
        .contextMenu {
            Button("Open") {
                viewing = artifact
            }
            if artifact.capability == .image {
                Button("Copy") {
                    shell.gallery.copy(artifact)
                }
            }
            Button("Download…") {
                shell.gallery.download(artifact)
            }
            Divider()
            Button("Delete…", role: .destructive) {
                deleting = artifact
            }
        }
        .accessibilityLabel(Provenance.prompt(of: artifact.params) ?? "Untitled image")
    }

    private func thumbnail(_ artifact: Artifact) -> some View {
        Color.clear
            .aspectRatio(1, contentMode: .fit)
            .frame(maxWidth: .infinity)
            .overlay {
                if let image = shell.gallery.thumbnail(artifact) {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                } else {
                    placeholder(artifact)
                }
            }
            .clipShape(RoundedRectangle.soft(Design.Radius.card))
            .overlay(
                RoundedRectangle.soft(Design.Radius.card)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
    }

    private func placeholder(_ artifact: Artifact) -> some View {
        ZStack {
            Rectangle().fill(Design.cardFill)
            if artifact.capability == .image {
                SkeletonPulse()
            } else {
                Image(systemName: typeGlyph(artifact))
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkFaint)
            }
        }
    }

    private func caption(_ artifact: Artifact) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                .font(Design.body.weight(.medium))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: Design.Space.m) {
                Text(artifact.createdAt.formatted(.relative(presentation: .named)))
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                Spacer(minLength: Design.Space.xs)
                TintChip(text: typeLabel(artifact), glyph: typeGlyph(artifact), faint: true)
            }
        }
        .padding(.horizontal, Design.Space.xs)
        .padding(.bottom, Design.Space.xxs)
    }

    private func actions(_ artifact: Artifact) -> some View {
        HStack(spacing: Design.Space.xxs) {
            GalleryQuickAction(glyph: "arrow.up.backward.and.arrow.down.forward", label: "Open") {
                viewing = artifact
            }
            if artifact.capability == .image {
                GalleryQuickAction(glyph: "doc.on.doc", label: "Copy") {
                    shell.gallery.copy(artifact)
                }
            }
            GalleryQuickAction(glyph: "arrow.down.to.line", label: "Save") {
                shell.gallery.download(artifact)
            }
            GalleryQuickAction(glyph: "trash", label: "Delete", destructive: true) {
                deleting = artifact
            }
        }
        .padding(Design.Space.xxs + 1)
        .background(Design.paper.opacity(0.94), in: RoundedRectangle.soft(Design.Radius.control))
        .overlay(
            RoundedRectangle.soft(Design.Radius.control)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .shade(Design.Elevation.button)
    }

    private func typeLabel(_ artifact: Artifact) -> String {
        switch artifact.capability {
        case .image: "Image"
        case .speak: "Voice"
        case .transcribe: "Audio"
        default: artifact.capability.rawValue.capitalized
        }
    }

    private func typeGlyph(_ artifact: Artifact) -> String {
        switch artifact.capability {
        case .image: "photo"
        case .speak, .transcribe: "waveform"
        default: "shippingbox"
        }
    }
}

struct GalleryImageViewer: View {
    @Bindable var shell: ShellModel
    let artifact: Artifact
    let onClose: () -> Void
    let onDelete: () -> Void
    @State private var image: NSImage?
    @State private var loadFailed = false

    var body: some View {
        ZStack {
            Design.shadowColor.opacity(0.72)
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture(perform: onClose)
                .accessibilityLabel("Dismiss")
            VStack(spacing: Design.Space.l) {
                imageStage
                footer
            }
            .padding(Design.Space.xl)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .overlay(alignment: .topTrailing) {
            SheetCloseButton(action: onClose)
                .padding(Design.Space.l)
        }
        .onExitCommand(perform: onClose)
        .task(id: artifact.id) {
            loadFailed = false
            image = shell.gallery.thumbnail(artifact)
            image = await shell.gallery.fullImage(artifact) ?? image
            loadFailed = image == nil
        }
    }

    private var imageStage: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFit()
                    .contentShape(Rectangle())
                    .onTapGesture {}
            } else if loadFailed {
                VStack(spacing: Design.Space.m) {
                    Image(systemName: "photo.badge.exclamationmark")
                        .font(Design.glyphPrimary)
                        .foregroundStyle(Design.inkSoft)
                    Text("Couldn't load this image.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                }
                .frame(width: 240, height: 180)
                .background(Design.cardFill, in: RoundedRectangle.soft(Design.Radius.artifact))
            } else {
                RoundedRectangle.soft(Design.Radius.artifact)
                    .fill(Design.cardFill)
                    .overlay(SkeletonPulse())
                    .frame(width: 240, height: 180)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var footer: some View {
        VStack(spacing: Design.Space.m) {
            Text(Provenance.prompt(of: artifact.params) ?? "Untitled")
                .font(Design.body.weight(.medium))
                .foregroundStyle(Design.ink)
                .multilineTextAlignment(.center)
                .lineLimit(2)
                .frame(maxWidth: Design.Bubble.promptMax)
            HStack(spacing: Design.Space.s) {
                if artifact.capability == .image {
                    GalleryQuickAction(glyph: "doc.on.doc", label: "Copy") {
                        shell.gallery.copy(artifact)
                    }
                }
                GalleryQuickAction(glyph: "arrow.down.to.line", label: "Save") {
                    shell.gallery.download(artifact)
                }
                GalleryQuickAction(
                    glyph: "trash", label: "Delete", destructive: true, action: onDelete)
            }
        }
    }
}

private struct GalleryQuickAction: View {
    let glyph: String
    let label: String
    var destructive = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: glyph)
                .font(Design.glyphSmall)
                .foregroundStyle(
                    destructive && hovering
                        ? Design.danger : hovering ? Design.ink : Design.inkSoft
                )
                .frame(width: 26, height: 26)
                .background(
                    hovering ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Color.clear),
                    in: RoundedRectangle.soft(Design.Radius.control))
                .contentShape(RoundedRectangle.soft(Design.Radius.control))
                .animation(Design.wash, value: hovering)
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}
