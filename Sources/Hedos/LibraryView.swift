import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class LibraryViewModel {
    let kernel: Kernel

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    var summary: DiscoverySummary?
    var records: [ModelRecord] = [] {
        didSet {
            shelfSignature = records.map {
                "\($0.id)|\($0.state.rawValue)|\(Launcher.destination(for: $0).rawValue)"
            }
        }
    }

    private(set) var shelfSignature: [String] = []
    var watchedFolders: [String] = []
    var hfCacheRoots: [String] = []
    var isScanning = false
    var errorMessage: String?

    func rescan() async {
        guard !isScanning else { return }
        isScanning = true
        defer { isScanning = false }
        do {
            summary = try await kernel.discover()
            records = try await kernel.shelf()
            watchedFolders = try await kernel.watchedFolders()
            hfCacheRoots = try await kernel.hfCacheRoots()
            errorMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func refreshShelf() async {
        records = (try? await kernel.shelf()) ?? records
    }

    func confirmRuntime(_ id: String) async {
        try? await kernel.confirmRuntime(id)
        await refreshShelf()
    }

    func overrideRuntime(_ id: String, to runtimeID: String) async {
        try? await kernel.overrideRuntime(id, to: runtimeID)
        await refreshShelf()
    }

    func addFolder(_ url: URL) async {
        try? await kernel.addWatchedFolder(url.path)
        await rescan()
    }

    func removeFolder(_ path: String) async {
        try? await kernel.removeWatchedFolder(path)
        await rescan()
    }

    func addHFRoot(_ url: URL) async {
        try? await kernel.addHFCacheRoot(url.path)
        await rescan()
    }

    func removeHFRoot(_ path: String) async {
        try? await kernel.removeHFCacheRoot(path)
        await rescan()
    }

    var endpointRecords: [ModelRecord] {
        records.filter { $0.source.kind == .endpoint }
    }

    private var liveTask: Task<Void, Never>?

    func startLiveUpdates() {
        liveTask?.cancel()
        let kernel = kernel
        liveTask = Task { [weak self] in
            for await summary in await kernel.shelfUpdates() {
                guard let self else { break }
                self.summary = summary
                self.records = (try? await kernel.shelf()) ?? self.records
            }
        }
    }

    func connectServer(baseURL: String, apiKey: String?) async -> (
        models: [String]?, error: String?
    ) {
        do {
            return (try await kernel.addServer(baseURL: baseURL, apiKey: apiKey), nil)
        } catch {
            return (nil, error.localizedDescription)
        }
    }

    @discardableResult
    func addEndpoint(baseURL: String, model: String) async -> Bool {
        do {
            _ = try await kernel.registerEndpoint(baseURL: baseURL, model: model)
            await refreshShelf()
            return true
        } catch {
            return false
        }
    }

    func removeEndpoint(id: String) async {
        try? await kernel.removeEndpoint(id)
        await refreshShelf()
    }

    func record(id: String?) -> ModelRecord? {
        guard let id else { return nil }
        return records.first { $0.id == id }
    }

    static func grouped(_ records: [ModelRecord]) -> [(section: String, records: [ModelRecord])] {
        let sections: [(SourceKind, String)] = [
            (.ollama, "Ollama"),
            (.huggingfaceCache, "Hugging Face"),
            (.lmStudio, "LM Studio"),
            (.builtin, "Built in"),
            (.endpoint, "Servers"),
            (.file, "Loose"),
            (.folder, "Loose"),
        ]
        var grouped: [String: [ModelRecord]] = [:]
        var order: [String] = []
        for (kind, title) in sections {
            let matching = records.filter { $0.source.kind == kind }
            guard !matching.isEmpty else { continue }
            if grouped[title] == nil { order.append(title) }
            grouped[title, default: []].append(contentsOf: matching)
        }
        let known = Set(sections.map(\.0))
        let other = records.filter { !known.contains($0.source.kind) }
        if !other.isEmpty {
            order.append("Other")
            grouped["Other"] = other
        }
        return order.map { ($0, grouped[$0] ?? []) }
    }
}

enum MetaGrid {
    static func tierWord(_ tier: RunTier) -> String {
        switch tier {
        case .native: "native"
        case .managed: "managed"
        case .remote: "remote"
        case .recipeNeeded: "needs recipe"
        }
    }
}

struct FoldersPopover: View {
    let model: LibraryViewModel
    var onOpenSettings: (() -> Void)? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Watched folders")
            if model.watchedFolders.isEmpty {
                Text("No extra folders yet.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.xs)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                ForEach(model.watchedFolders, id: \.self) { path in
                    FolderRow(path: path) {
                        Task { await model.removeFolder(path) }
                    }
                }
            }
            Button("Add Folder…") {
                FolderRow.pickFolder { url in
                    Task { await model.addFolder(url) }
                }
            }
            .buttonStyle(QuietButtonStyle())
            Rectangle()
                .fill(Design.hairline)
                .frame(height: Design.hairlineWidth)
                .padding(.vertical, Design.Space.xs)
            Text("Hedos scans Downloads, Models, and any folders added here.")
                .font(Design.label)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(2)
            if let onOpenSettings {
                Button("Manage in Settings…") {
                    onOpenSettings()
                }
                .buttonStyle(QuietButtonStyle())
            }
        }
        .padding(Design.Space.xl)
        .frame(width: Design.Popover.form.width)
        .background(Design.paper)
    }
}

struct FolderRow: View {
    let path: String
    let onRemove: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: Design.Space.chipX) {
            SourceMark(kind: .folder, size: 12)
                .foregroundStyle(Design.inkSoft)
            Text((path as NSString).abbreviatingWithTildeInPath)
                .font(Design.data(11))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: Design.Space.m)
            Button(action: onRemove) {
                Image(systemName: "xmark.circle.fill")
                    .font(Design.glyphInline)
                    .foregroundStyle(hovering ? Design.inkSoft : Design.inkFaint)
            }
            .buttonStyle(PressDipStyle())
            .help("Stop watching")
            .accessibilityLabel("Stop watching folder")
        }
        .padding(.horizontal, Design.Space.m)
        .padding(.vertical, Design.Space.s)
        .background(
            RoundedRectangle(cornerRadius: Design.Radius.control)
                .fill(hovering ? Design.inkWash : .clear))
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
    }

    static func pickFolder(_ onPick: @escaping (URL) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Watch Folder"
        if panel.runModal() == .OK, let url = panel.url {
            onPick(url)
        }
    }
}

