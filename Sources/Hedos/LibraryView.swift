import AppKit
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class LibraryViewModel {
    let kernel = Kernel()

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

    var groupedRecords: [(section: String, records: [ModelRecord])] {
        let sections: [(SourceKind, String)] = [
            (.ollama, "Ollama"),
            (.huggingfaceCache, "Hugging Face cache"),
            (.lmStudio, "LM Studio"),
            (.file, "Loose files"),
            (.folder, "Loose files"),
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

struct LibraryView: View {
    @State private var model = LibraryViewModel()
    @State private var selectedID: String?
    @State private var showFolders = false

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 290, ideal: 340)
        } detail: {
            detail
        }
        .frame(minWidth: 800, minHeight: 500)
        .tint(Design.accent)
        .task { await model.rescan() }
    }

    private var sidebar: some View {
        List(selection: $selectedID) {
            ForEach(model.groupedRecords, id: \.section) { group in
                Section {
                    ForEach(group.records) { record in
                        ModelRow(record: record).tag(record.id)
                    }
                } header: {
                    Text(group.section)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .textCase(nil)
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) { footer }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.rescan() }
                } label: {
                    Label("Rescan", systemImage: "arrow.clockwise")
                }
                .help("Scan the machine again")
                .disabled(model.isScanning)
            }
        }
    }

    private var footer: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(spacing: 8) {
                if model.isScanning {
                    ProgressView().controlSize(.mini)
                    Text("Scanning…").font(.caption).foregroundStyle(.secondary)
                } else if let summary = model.summary {
                    Text("\(summary.totalCount) models")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                    Text(DiscoverySummary.formatBytes(summary.totalBytes))
                        .font(Design.data(10))
                        .foregroundStyle(.tertiary)
                    if !summary.duplicates.isEmpty {
                        Image(systemName: "externaldrive.badge.exclamationmark")
                            .font(.caption)
                            .foregroundStyle(Design.terracotta)
                            .help("Duplicate weights on disk")
                    }
                }
                Spacer()
                Button {
                    showFolders.toggle()
                } label: {
                    Image(systemName: "folder.badge.plus")
                        .font(.caption)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Watched folders")
                .popover(isPresented: $showFolders, arrowEdge: .bottom) {
                    FoldersPopover(model: model)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
        }
        .background(.bar)
    }

    @ViewBuilder
    private var detail: some View {
        if let record = model.record(id: selectedID) {
            if record.runtime.tier == .recipeNeeded {
                RecipeNeededPane(record: record, shelf: model.records)
            } else if record.capabilities.contains(.chat), record.runtime.id != nil {
                ChatView(record: record, kernel: model.kernel)
                    .id(record.id)
                    .sheet(isPresented: needsConfirmation(record)) {
                        ResolutionSheet(record: record, library: model)
                    }
            } else if record.capabilities.contains(.speak), record.runtime.id != nil {
                VoiceView(record: record, kernel: model.kernel)
                    .id(record.id)
                    .sheet(isPresented: needsConfirmation(record)) {
                        ResolutionSheet(record: record, library: model)
                    }
            } else {
                ModelInfoPane(record: record)
            }
        } else {
            HeroPane(model: model)
        }
    }

    private func needsConfirmation(_ record: ModelRecord) -> Binding<Bool> {
        Binding(
            get: {
                let current = model.record(id: record.id) ?? record
                return current.runtime.resolved == .auto && current.runtime.confirmedAt == nil
            },
            set: { _ in })
    }
}

struct ModelRow: View {
    let record: ModelRecord

    var body: some View {
        HStack(spacing: 9) {
            RoundedRectangle(cornerRadius: 6)
                .fill(Design.modalityColor(record.modality).opacity(0.16))
                .frame(width: 24, height: 24)
                .overlay {
                    Image(systemName: Design.modalityGlyph(record.modality))
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Design.modalityColor(record.modality))
                }
            VStack(alignment: .leading, spacing: 1) {
                Text(record.name).lineLimit(1)
                if let repo = record.source.repo, repo != record.name {
                    Text(repo)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            .layoutPriority(1)
            Spacer(minLength: 6)
            if record.state == .missing {
                Image(systemName: "questionmark.circle")
                    .font(.caption)
                    .foregroundStyle(Design.terracotta)
                    .help("No longer found on disk")
            } else {
                TierBadge(tier: record.runtime.tier)
            }
            if let mb = record.footprintMB, mb > 0 {
                Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                    .font(Design.data(10))
                    .foregroundStyle(.tertiary)
                    .frame(minWidth: 46, alignment: .trailing)
            }
        }
        .padding(.vertical, 2)
    }
}

struct TierBadge: View {
    let tier: RunTier

    var body: some View {
        Text(label)
            .font(.system(size: 9, weight: .semibold))
            .tracking(0.3)
            .padding(.horizontal, 5)
            .padding(.vertical, 1.5)
            .background(color.opacity(0.15), in: Capsule())
            .foregroundStyle(color)
    }

    private var label: String {
        switch tier {
        case .native: "NATIVE"
        case .managed: "MANAGED"
        case .recipeNeeded: "RECIPE"
        }
    }

    private var color: Color {
        switch tier {
        case .native: Design.laurel
        case .managed: Design.lapis
        case .recipeNeeded: Design.granite
        }
    }
}

struct HeroPane: View {
    let model: LibraryViewModel

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            HeptagonMark(size: 56, color: .primary.opacity(0.85))
                .padding(.bottom, 28)
            if let summary = model.summary {
                Text(summary.headline)
                    .font(Design.plaque(21))
                    .multilineTextAlignment(.center)
                    .lineSpacing(5)
                    .frame(maxWidth: 460)
                ForEach(summary.duplicates, id: \.self) { group in
                    Text(
                        "\(group.names.joined(separator: " and ")) live in more than one place — \(DiscoverySummary.formatBytes(group.wastedBytes)) duplicated."
                    )
                    .font(.callout)
                    .foregroundStyle(Design.terracotta)
                    .padding(.top, 14)
                }
                Text("Select a model to open it.")
                    .font(.callout)
                    .foregroundStyle(.tertiary)
                    .padding(.top, 22)
            } else {
                Text("Looking for models on this Mac…")
                    .font(Design.plaque(21))
                    .foregroundStyle(.secondary)
                ProgressView()
                    .controlSize(.small)
                    .padding(.top, 16)
            }
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(32)
    }
}

struct FoldersPopover: View {
    let model: LibraryViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Watched folders")
                .font(.callout.weight(.semibold))
            Text("Hedos scans Downloads, Models, and any folders you add here.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if !model.watchedFolders.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(model.watchedFolders, id: \.self) { path in
                        HStack(spacing: 6) {
                            Image(systemName: "folder")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text((path as NSString).abbreviatingWithTildeInPath)
                                .font(.caption)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Button {
                                Task { await model.removeFolder(path) }
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .font(.caption)
                                    .foregroundStyle(.tertiary)
                            }
                            .buttonStyle(.plain)
                            .help("Stop watching")
                        }
                    }
                }
                .padding(.vertical, 2)
            }
            Button {
                let panel = NSOpenPanel()
                panel.canChooseDirectories = true
                panel.canChooseFiles = false
                panel.allowsMultipleSelection = false
                panel.prompt = "Watch Folder"
                if panel.runModal() == .OK, let url = panel.url {
                    Task { await model.addFolder(url) }
                }
            } label: {
                Label("Add folder…", systemImage: "plus")
                    .font(.callout)
            }
        }
        .padding(14)
        .frame(width: 300)
    }
}

struct ResolutionSheet: View {
    let record: ModelRecord
    let library: LibraryViewModel
    @State private var chosen: String

    init(record: ModelRecord, library: LibraryViewModel) {
        self.record = record
        self.library = library
        _chosen = State(initialValue: record.runtime.id ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                RoundedRectangle(cornerRadius: 7)
                    .fill(Design.modalityColor(record.modality).opacity(0.16))
                    .frame(width: 30, height: 30)
                    .overlay {
                        Image(systemName: Design.modalityGlyph(record.modality))
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(Design.modalityColor(record.modality))
                    }
                Text("Run \(record.name)?")
                    .font(.title3.weight(.semibold))
            }
            Text(
                "Hedos will run this \(record.modality.rawValue) model via **\(record.runtime.id ?? "?")**."
            )
            .foregroundStyle(.secondary)
            if !record.runtime.alternatives.isEmpty {
                Picker("Runtime", selection: $chosen) {
                    Text(record.runtime.id ?? "?").tag(record.runtime.id ?? "")
                    ForEach(record.runtime.alternatives, id: \.self) { alt in
                        Text(alt).tag(alt)
                    }
                }
                .pickerStyle(.radioGroup)
            }
            HStack {
                Spacer()
                Button("Run") {
                    Task {
                        if chosen == record.runtime.id ?? "" {
                            await library.confirmRuntime(record.id)
                        } else {
                            await library.overrideRuntime(record.id, to: chosen)
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 380)
    }
}

struct ModelInfoPane: View {
    let record: ModelRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Text(record.name).font(Design.plaque(24))
                TierBadge(tier: record.runtime.tier)
            }
            Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 7) {
                GridRow {
                    Text("Kind").foregroundStyle(.secondary)
                    Text(record.source.kind.rawValue)
                }
                GridRow {
                    Text("Modality").foregroundStyle(.secondary)
                    Text(record.modality.rawValue)
                }
                if let repo = record.source.repo {
                    GridRow {
                        Text("Repo").foregroundStyle(.secondary)
                        Text(repo)
                    }
                }
                if let mb = record.footprintMB, mb > 0 {
                    GridRow {
                        Text("Size").foregroundStyle(.secondary)
                        Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                            .font(Design.data(12))
                    }
                }
                if let runtime = record.runtime.id {
                    GridRow {
                        Text("Runtime").foregroundStyle(.secondary)
                        Text(runtime)
                    }
                }
            }
            .font(.callout)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(28)
    }
}

struct RecipeNeededPane: View {
    let record: ModelRecord
    let shelf: [ModelRecord]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Text(record.name).font(Design.plaque(24))
                TierBadge(tier: .recipeNeeded)
            }
            Text(reason)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let sibling = runnableSibling {
                Label(
                    "You also have \(sibling.name), which runs.",
                    systemImage: "checkmark.circle")
                .foregroundStyle(Design.laurel)
            }
            Text(
                "A runtime recipe — a small manifest that teaches Hedos how to execute this format — can make it runnable later."
            )
            .font(.caption)
            .foregroundStyle(.tertiary)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(28)
    }

    private var reason: String {
        let identified = Identification.identify(record)
        switch identified.format {
        case .unknown:
            return record.primaryWeightPath == nil
                ? "Hedos found this model, but its weights are in a format none of the built-in runtimes can execute — no safetensors or GGUF weights detected, likely PyTorch or another framework's format."
                : "Hedos found this model but could not identify what kind it is."
        case .diffusers:
            return
                "This is an image-generation pipeline. Hedos's image runtime arrives in a later milestone."
        case .safetensors, .mlxSafetensors:
            return
                "This model's format is recognized, but no built-in runtime serves its modality (\(record.modality.rawValue)) yet."
        default:
            return "No built-in runtime can execute this model yet."
        }
    }

    private var runnableSibling: ModelRecord? {
        let base =
            record.name.lowercased().split(separator: "-").first.map(String.init)
            ?? record.name.lowercased()
        guard base.count >= 4 else { return nil }
        return shelf.first {
            $0.id != record.id && $0.state == .ready
                && $0.name.lowercased().contains(base)
        }
    }
}
