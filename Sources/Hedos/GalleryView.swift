import AppKit
import HedosKernel
import ImageIO
import SwiftUI

@Observable
@MainActor
final class GalleryViewModel {
    private let kernel: Kernel
    private var thumbnails: [String: NSImage] = [:]
    private var selectedFullImage: (id: String, image: NSImage)?

    var artifacts: [Artifact] = []
    var filterModelID: String?
    var sort: GallerySort = .newestFirst
    var selectedID: String?
    var notice: String?
    var isRunning = false

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    var arranged: [Artifact] {
        Gallery.arrange(artifacts, modelID: filterModelID, sort: sort)
    }

    var models: [GalleryModel] {
        Gallery.models(in: artifacts)
    }

    var selected: Artifact? {
        guard let selectedID else { return nil }
        return artifacts.first { $0.id == selectedID }
    }

    func load() async {
        artifacts = (try? await kernel.artifacts()) ?? []
        if let filterModelID, !artifacts.contains(where: { $0.modelID == filterModelID }) {
            self.filterModelID = nil
        }
        if let selectedID, !artifacts.contains(where: { $0.id == selectedID }) {
            self.selectedID = nil
        }
    }

    func toggleSelection(_ artifact: Artifact) {
        selectedID = selectedID == artifact.id ? nil : artifact.id
        notice = nil
    }

    func thumbnail(_ artifact: Artifact) -> NSImage? {
        thumbnails[artifact.id]
    }

    func loadThumbnail(_ artifact: Artifact) async {
        guard thumbnails[artifact.id] == nil else { return }
        if let data = try? await kernel.artifactPreview(id: artifact.id),
            let image = NSImage(data: data)
        {
            thumbnails[artifact.id] = image
            return
        }
        guard let url = try? await kernel.artifactURL(id: artifact.id),
            let image = Self.downsampled(url, maxPixel: 480)
        else { return }
        thumbnails[artifact.id] = image
    }

    func fullImage(_ artifact: Artifact) -> NSImage? {
        guard let selectedFullImage, selectedFullImage.id == artifact.id else { return nil }
        return selectedFullImage.image
    }

    func loadFullImage(_ artifact: Artifact) async {
        guard selectedFullImage?.id != artifact.id else { return }
        guard let url = try? await kernel.artifactURL(id: artifact.id),
            let image = NSImage(contentsOf: url)
        else { return }
        guard selectedID == artifact.id else { return }
        selectedFullImage = (artifact.id, image)
    }

    func delete(_ artifact: Artifact) async {
        do {
            try await kernel.deleteArtifact(id: artifact.id)
            thumbnails[artifact.id] = nil
            if selectedFullImage?.id == artifact.id {
                selectedFullImage = nil
            }
            if selectedID == artifact.id {
                selectedID = nil
            }
            await load()
        } catch {
            notice = error.localizedDescription
        }
    }

    func reveal(_ artifact: Artifact) {
        let kernel = kernel
        Task {
            guard let url = try? await kernel.artifactURL(id: artifact.id) else { return }
            NSWorkspace.shared.activateFileViewerSelecting([url])
        }
    }

    func rerun(_ artifact: Artifact) {
        let kernel = kernel
        run {
            try await kernel.rerun(artifactID: artifact.id)
        }
    }

    func vary(_ artifact: Artifact) {
        let kernel = kernel
        run {
            try await kernel.vary(artifactID: artifact.id)
        }
    }

    private func run(_ submit: @escaping @Sendable () async throws -> String) {
        guard !isRunning else { return }
        isRunning = true
        notice = nil
        Task { [weak self] in
            guard let self else { return }
            do {
                let jobID = try await submit()
                for await event in await kernel.jobEvents(id: jobID) {
                    if case .failed(let message) = event {
                        notice = message
                    }
                }
                await load()
                if let job = try? await kernel.job(id: jobID), let landed = job.result.first {
                    selectedID = landed
                }
            } catch {
                notice = error.localizedDescription
            }
            isRunning = false
        }
    }

    private static func downsampled(_ url: URL, maxPixel: CGFloat) -> NSImage? {
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

struct GalleryView: View {
    let shelf: [ModelRecord]
    let onOpenParams: (Artifact) -> Void
    @State private var model: GalleryViewModel

    init(kernel: Kernel, shelf: [ModelRecord], onOpenParams: @escaping (Artifact) -> Void) {
        self.shelf = shelf
        self.onOpenParams = onOpenParams
        _model = State(initialValue: GalleryViewModel(kernel: kernel))
    }

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                header
                Divider()
                grid
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let artifact = model.selected {
                Divider()
                detailPane(artifact)
            }
        }
        .navigationTitle("Gallery")
        .navigationSubtitle(subtitle)
        .task { await model.load() }
    }

    private var subtitle: String {
        let count = model.artifacts.count
        switch count {
        case 0: return ""
        case 1: return "1 image"
        default: return "\(count) images"
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Picker("Model", selection: $model.filterModelID) {
                Text("All models").tag(String?.none)
                ForEach(model.models) { entry in
                    Text(entry.name).tag(String?.some(entry.id))
                }
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .controlSize(.small)
            .fixedSize()
            .disabled(model.models.isEmpty)
            Picker("Sort", selection: $model.sort) {
                Text("Newest first").tag(GallerySort.newestFirst)
                Text("Oldest first").tag(GallerySort.oldestFirst)
            }
            .pickerStyle(.menu)
            .labelsHidden()
            .controlSize(.small)
            .fixedSize()
            .disabled(model.artifacts.isEmpty)
            Spacer()
            if model.isRunning {
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.mini)
                    Text("Running…")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
    }

    @ViewBuilder
    private var grid: some View {
        if model.artifacts.isEmpty {
            emptyState
        } else if model.arranged.isEmpty {
            VStack(spacing: 8) {
                Text("No images from this model.")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 132, maximum: 200), spacing: 10)],
                    spacing: 10
                ) {
                    ForEach(model.arranged) { artifact in
                        GalleryCell(
                            artifact: artifact,
                            image: model.thumbnail(artifact),
                            provenance: Provenance.line(
                                for: artifact, schema: schema(for: artifact)),
                            isSelected: model.selectedID == artifact.id
                        ) {
                            model.toggleSelection(artifact)
                        }
                        .task(id: artifact.id) {
                            await model.loadThumbnail(artifact)
                        }
                    }
                }
                .padding(14)
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 14) {
            Image(systemName: "photo.stack")
                .font(.system(size: 26, weight: .light))
                .foregroundStyle(.quaternary)
            Text("Nothing generated yet.")
                .font(Design.plaque(16))
                .foregroundStyle(.secondary)
            Text("Images you generate land here, each with its full provenance.")
                .font(.system(size: 12))
                .foregroundStyle(.tertiary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(28)
    }

    private func detailPane(_ artifact: Artifact) -> some View {
        GalleryDetailPane(
            artifact: artifact,
            schema: schema(for: artifact),
            canRun: canRun(artifact),
            model: model,
            onOpenParams: onOpenParams)
    }

    private func schema(for artifact: Artifact) -> [ParamSpec] {
        shelf.first { $0.id == artifact.modelID }?.params ?? []
    }

    private func canRun(_ artifact: Artifact) -> Bool {
        guard let record = shelf.first(where: { $0.id == artifact.modelID }) else { return false }
        return record.runtime.id != nil && record.capabilities.contains(artifact.capability)
    }
}

private struct GalleryCell: View {
    let artifact: Artifact
    let image: NSImage?
    let provenance: String
    let isSelected: Bool
    let onSelect: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: onSelect) {
            ZStack(alignment: .bottom) {
                thumb
                if hovering {
                    overlay
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(
                        isSelected ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.quaternary),
                        lineWidth: isSelected ? 2 : 1))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(hoverHelp)
        .accessibilityLabel(accessibilityText)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    @ViewBuilder
    private var thumb: some View {
        if let image {
            Color.clear
                .aspectRatio(1, contentMode: .fit)
                .overlay(
                    Image(nsImage: image)
                        .resizable()
                        .interpolation(.medium)
                        .scaledToFill()
                )
                .clipped()
        } else {
            RoundedRectangle(cornerRadius: 8)
                .fill(.quaternary.opacity(0.3))
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    Image(systemName: "photo")
                        .font(.system(size: 15, weight: .light))
                        .foregroundStyle(.quaternary)
                }
        }
    }

    private var overlay: some View {
        VStack(alignment: .leading, spacing: 2) {
            if let prompt = Provenance.prompt(of: artifact.params) {
                Text(prompt)
                    .font(.system(size: 10))
                    .lineLimit(1)
            }
            Text(provenance)
                .font(Design.data(9))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 7)
        .padding(.vertical, 5)
        .background(.ultraThinMaterial)
    }

    private var hoverHelp: String {
        if let prompt = Provenance.prompt(of: artifact.params) {
            return "\(prompt)\n\(provenance)"
        }
        return provenance
    }

    private var accessibilityText: String {
        if let prompt = Provenance.prompt(of: artifact.params) {
            return "Generated image: \(prompt)"
        }
        return "Generated image"
    }
}

private struct GalleryDetailPane: View {
    let artifact: Artifact
    let schema: [ParamSpec]
    let canRun: Bool
    let model: GalleryViewModel
    let onOpenParams: (Artifact) -> Void
    @State private var confirmingDelete = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                fullImage
                if let prompt = Provenance.prompt(of: artifact.params) {
                    Text(prompt)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                provenance
                if let notice = model.notice {
                    Text(notice)
                        .font(.system(size: 11))
                        .foregroundStyle(Design.warn)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                actions
            }
            .padding(16)
        }
        .frame(width: 300)
        .task(id: artifact.id) {
            await model.loadFullImage(artifact)
        }
        .confirmationDialog(
            "Move this image to the Trash?",
            isPresented: $confirmingDelete
        ) {
            Button("Move to Trash", role: .destructive) {
                Task { await model.delete(artifact) }
            }
        } message: {
            Text("The file moves to the Trash — it is not deleted outright.")
        }
    }

    @ViewBuilder
    private var fullImage: some View {
        if let image = model.fullImage(artifact) {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .scaledToFit()
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .accessibilityLabel(
                    Provenance.prompt(of: artifact.params).map { "Generated image: \($0)" }
                        ?? "Generated image")
        } else {
            RoundedRectangle(cornerRadius: 8)
                .fill(.quaternary.opacity(0.3))
                .aspectRatio(1, contentMode: .fit)
                .overlay {
                    ProgressView()
                        .controlSize(.small)
                }
        }
    }

    private var provenance: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(Provenance.details(for: artifact, schema: schema))
                .font(Design.data(11))
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Text(artifact.createdAt.formatted(date: .abbreviated, time: .shortened))
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
        }
    }

    private var actions: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Button("Re-run") {
                    model.rerun(artifact)
                }
                .help("Generate again with the identical seed")
                Button("Vary") {
                    model.vary(artifact)
                }
                .help("Generate a sibling with a fresh seed")
            }
            .disabled(!canRun || model.isRunning)
            Button("Open params in canvas") {
                onOpenParams(artifact)
            }
            .disabled(!canRun)
            .help(
                canRun
                    ? "Load this image's params into the canvas"
                    : "This model is no longer runnable on the shelf")
            HStack(spacing: 10) {
                Button("Reveal in Finder") {
                    model.reveal(artifact)
                }
                Button("Delete", role: .destructive) {
                    confirmingDelete = true
                }
                .help("Move the file to the Trash")
            }
        }
        .controlSize(.small)
    }
}
