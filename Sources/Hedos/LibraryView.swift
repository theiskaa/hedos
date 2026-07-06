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

    var groupedRecords: [(section: String, records: [ModelRecord])] {
        Self.grouped(records)
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

struct CanvasPrefill: Identifiable {
    let id = UUID()
    let artifact: Artifact
}

struct LibrarySidebar: View {
    let model: LibraryViewModel
    @Binding var selection: String?
    @State private var showFolders = false

    var body: some View {
        List(selection: $selection) {
            ForEach(model.groupedRecords, id: \.section) { group in
                Section {
                    ForEach(group.records) { record in
                        ModelRow(record: record)
                            .contentShape(Rectangle())
                            .tag(record.id)
                    }
                } header: {
                    Text(group.section.uppercased())
                        .font(.system(size: 10, weight: .medium))
                        .tracking(0.8)
                        .foregroundStyle(.tertiary)
                }
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("Library")
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
            HStack(spacing: 6) {
                if model.isScanning {
                    ProgressView().controlSize(.mini)
                } else if let summary = model.summary {
                    Text("\(summary.totalCount) models · \(DiscoverySummary.formatBytes(summary.totalBytes))")
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                    if !summary.duplicates.isEmpty {
                        Circle()
                            .fill(Design.warn)
                            .frame(width: 5, height: 5)
                            .help(duplicatesSummary(summary))
                    }
                }
                Spacer()
                Button {
                    showFolders.toggle()
                } label: {
                    Image(systemName: "folder.badge.plus")
                        .font(.system(size: 11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.tertiary)
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

    private func duplicatesSummary(_ summary: DiscoverySummary) -> String {
        summary.duplicates.map {
            "\($0.names.joined(separator: " and ")) are duplicates — \(DiscoverySummary.formatBytes($0.wastedBytes)) wasted"
        }.joined(separator: "\n")
    }
}

struct LibraryDetail: View {
    let model: LibraryViewModel
    let selectedID: String?

    var body: some View {
        if let record = model.record(id: selectedID) {
            if record.runtime.tier == .recipeNeeded {
                RecipeNeededPane(record: record, shelf: model.records)
            } else {
                ModelInfoPane(record: record)
            }
        } else {
            HeroPane(model: model)
        }
    }
}

struct ModelRow: View {
    let record: ModelRecord

    var body: some View {
        HStack {
            Text(record.name)
                .font(.system(size: 13))
                .lineLimit(1)
            Spacer(minLength: 8)
            if record.state == .missing {
                Circle()
                    .fill(Design.warn.opacity(0.8))
                    .frame(width: 5, height: 5)
                    .help("No longer found on disk")
            } else if record.runtime.tier == .recipeNeeded {
                Circle()
                    .fill(.quaternary)
                    .frame(width: 5, height: 5)
                    .help("Needs a runtime recipe — select for details")
            }
        }
        .padding(.vertical, 1)
    }
}

struct HeroPane: View {
    let model: LibraryViewModel

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            HeptagonMark(size: 52, color: .primary.opacity(0.85))
                .padding(.bottom, 30)
            if let summary = model.summary {
                Text(summary.headline)
                    .font(Design.plaque(20))
                    .multilineTextAlignment(.center)
                    .lineSpacing(5)
                    .frame(maxWidth: 430)
                Text("Select a model to open it.")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .padding(.top, 24)
            } else {
                Text("Looking for models on this Mac…")
                    .font(Design.plaque(20))
                    .foregroundStyle(.secondary)
                ProgressView()
                    .controlSize(.small)
                    .padding(.top, 18)
            }
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(32)
    }
}

struct DetailHeader: View {
    let record: ModelRecord
    var trailing: AnyView? = nil

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: Design.modalityGlyph(record.modality))
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Design.modalityColor(record.modality))
            VStack(alignment: .leading, spacing: 1) {
                Text(record.name)
                    .font(.system(size: 15, weight: .semibold))
                if let runtime = record.runtime.id {
                    Text("runs via \(runtime) · \(tierWord)")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
            }
            Spacer()
            if let trailing { trailing }
        }
    }

    private var tierWord: String {
        switch record.runtime.tier {
        case .native: "native"
        case .managed: "managed"
        case .recipeNeeded: "needs recipe"
        }
    }
}

struct MetaGrid: View {
    let record: ModelRecord

    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
            row("Kind", record.source.kind.rawValue)
            row("Modality", record.modality.rawValue)
            if let repo = record.source.repo {
                row("Repo", repo)
            }
            if let mb = record.footprintMB, mb > 0 {
                GridRow {
                    label("Size")
                    Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                        .font(Design.data(12))
                }
            }
        }
        .font(.system(size: 13))
    }

    private func row(_ name: String, _ value: String) -> some View {
        GridRow {
            label(name)
            Text(value)
        }
    }

    private func label(_ name: String) -> some View {
        Text(name)
            .font(.system(size: 11))
            .foregroundStyle(.tertiary)
    }
}

struct ModelInfoPane: View {
    let record: ModelRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            DetailHeader(record: record)
            MetaGrid(record: record)
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
        VStack(alignment: .leading, spacing: 22) {
            DetailHeader(record: record)
            Text(reason)
                .font(.system(size: 13))
                .foregroundStyle(.secondary)
                .lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 480, alignment: .leading)
            if let sibling = runnableSibling {
                Text("You also have \(sibling.name), which runs.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
            }
            MetaGrid(record: record)
            Spacer()
            Text("A runtime recipe can make this model runnable later.")
                .font(.system(size: 11))
                .foregroundStyle(.quaternary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(28)
    }

    private var reason: String {
        let identified = Identification.identify(record)
        switch identified.format {
        case .unknown:
            return record.primaryWeightPath == nil
                ? "This model's weights are in a format none of the built-in runtimes can execute — no safetensors or GGUF detected, likely PyTorch or another framework's format."
                : "Hedos found this model but could not identify what kind it is."
        case .diffusers:
            return "This is an image-generation pipeline the built-in image runtime cannot serve yet."
        case .safetensors, .mlxSafetensors:
            return "The format is recognized, but no built-in runtime serves \(record.modality.rawValue) models yet."
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

struct FoldersPopover: View {
    let model: LibraryViewModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Watched folders")
                .font(.system(size: 13, weight: .semibold))
            Text("Hedos scans Downloads, Models, and any folders added here.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if !model.watchedFolders.isEmpty {
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(model.watchedFolders, id: \.self) { path in
                        HStack(spacing: 7) {
                            Text((path as NSString).abbreviatingWithTildeInPath)
                                .font(.system(size: 11))
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Button {
                                Task { await model.removeFolder(path) }
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.system(size: 8, weight: .bold))
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
                Text("Add folder…")
                    .font(.system(size: 12))
            }
        }
        .padding(14)
        .frame(width: 280)
    }
}

struct ResolutionSheet: View {
    let record: ModelRecord
    let library: LibraryViewModel
    let onDismiss: (Bool) -> Void
    @State private var chosen: String

    init(record: ModelRecord, library: LibraryViewModel, onDismiss: @escaping (Bool) -> Void) {
        self.record = record
        self.library = library
        self.onDismiss = onDismiss
        _chosen = State(initialValue: record.runtime.id ?? "")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Run \(record.name)?")
                .font(.system(size: 15, weight: .semibold))
            Text("Hedos will run this \(record.modality.rawValue) model via \(record.runtime.id ?? "?").")
                .font(.system(size: 13))
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
                Button("Cancel") { onDismiss(false) }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button("Run") {
                    Task {
                        if chosen == record.runtime.id ?? "" {
                            await library.confirmRuntime(record.id)
                        } else {
                            await library.overrideRuntime(record.id, to: chosen)
                        }
                        onDismiss(true)
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 340)
    }
}
