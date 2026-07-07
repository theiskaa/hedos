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
    var records: [ModelRecord] = []
    var watchedFolders: [String] = []
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

    func record(id: String?) -> ModelRecord? {
        guard let id else { return nil }
        return records.first { $0.id == id }
    }

    static func grouped(_ records: [ModelRecord]) -> [(section: String, records: [ModelRecord])] {
        let sections: [(SourceKind, String)] = [
            (.ollama, "Ollama"),
            (.huggingfaceCache, "Hugging Face"),
            (.lmStudio, "LM Studio"),
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
        case .recipeNeeded: "needs recipe"
        }
    }
}

struct FoldersPopover: View {
    let model: LibraryViewModel
    var onOpenSettings: (() -> Void)? = nil

    var body: some View {
        Form {
            Section {
                ForEach(model.watchedFolders, id: \.self) { path in
                    LabeledContent {
                        Button {
                            Task { await model.removeFolder(path) }
                        } label: {
                            Image(systemName: "xmark")
                                .font(Design.glyphSmall.weight(.bold))
                                .foregroundStyle(Design.inkFaint)
                        }
                        .buttonStyle(.plain)
                        .help("Stop watching")
                        .accessibilityLabel("Stop watching folder")
                    } label: {
                        Text((path as NSString).abbreviatingWithTildeInPath)
                            .font(Design.label)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                }
                Button("Add Folder…") {
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.canChooseFiles = false
                    panel.allowsMultipleSelection = false
                    panel.prompt = "Watch Folder"
                    if panel.runModal() == .OK, let url = panel.url {
                        Task { await model.addFolder(url) }
                    }
                }
            } header: {
                Text("Watched folders")
            } footer: {
                VStack(alignment: .leading, spacing: Design.Space.s) {
                    Text("Hedos scans Downloads, Models, and any folders added here.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                    if let onOpenSettings {
                        Button("Manage in Settings…") {
                            onOpenSettings()
                        }
                        .buttonStyle(QuietButtonStyle())
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: Design.Popover.form.width)
        .frame(maxHeight: Design.Popover.form.height)
    }
}

