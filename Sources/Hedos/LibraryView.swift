import HedosKernel
import SwiftUI

@Observable
@MainActor
final class LibraryViewModel {
    let kernel = Kernel()

    var summary: DiscoverySummary?
    var records: [ModelRecord] = []
    var isScanning = false
    var errorMessage: String?

    func rescan() async {
        guard !isScanning else { return }
        isScanning = true
        defer { isScanning = false }
        do {
            summary = try await kernel.discover()
            records = try await kernel.shelf()
            errorMessage = nil
        } catch {
            errorMessage = String(describing: error)
        }
    }

    func record(id: String?) -> ModelRecord? {
        guard let id else { return nil }
        return records.first { $0.id == id }
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

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 280, ideal: 330)
        } detail: {
            detail
        }
        .frame(minWidth: 760, minHeight: 460)
        .task { await model.rescan() }
    }

    private var sidebar: some View {
        List(selection: $selectedID) {
            ForEach(model.groupedRecords, id: \.section) { group in
                Section(group.section) {
                    ForEach(group.records) { record in
                        row(record).tag(record.id)
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) { statusFooter }
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.rescan() }
                } label: {
                    Label("Rescan", systemImage: "arrow.clockwise")
                }
                .disabled(model.isScanning)
            }
        }
    }

    private var statusFooter: some View {
        VStack(alignment: .leading, spacing: 2) {
            Divider()
            HStack(spacing: 6) {
                if model.isScanning {
                    ProgressView().controlSize(.mini)
                    Text("Looking for models…")
                } else if let summary = model.summary {
                    Text("\(summary.totalCount) models · \(DiscoverySummary.formatBytes(summary.totalBytes))")
                    if !summary.duplicates.isEmpty {
                        Image(systemName: "externaldrive.badge.exclamationmark")
                            .foregroundStyle(.orange)
                            .help("Duplicate weights found — see details")
                    }
                }
                Spacer()
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
        }
        .background(.bar)
    }

    private func row(_ record: ModelRecord) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(record.name).lineLimit(1)
                if let repo = record.source.repo, repo != record.name {
                    Text(repo).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
            if record.state == .missing {
                Image(systemName: "questionmark.circle")
                    .foregroundStyle(.orange)
                    .help("No longer found on disk")
            } else {
                TierBadge(tier: record.runtime.tier)
            }
            if let mb = record.footprintMB, mb > 0 {
                Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let record = model.record(id: selectedID) {
            if record.runtime.tier == .recipeNeeded {
                RecipeNeededPane(record: record)
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
            VStack(spacing: 20) {
                Spacer()
                Image(systemName: "square.stack.3d.up")
                    .font(.system(size: 40))
                    .foregroundStyle(.tertiary)
                if let summary = model.summary {
                    Text(summary.headline)
                        .font(.title3.weight(.medium))
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 460)
                    ForEach(summary.duplicates, id: \.self) { group in
                        Label(
                            "\(group.names.joined(separator: " and ")) live in more than one place — \(DiscoverySummary.formatBytes(group.wastedBytes)) duplicated.",
                            systemImage: "externaldrive.badge.exclamationmark")
                        .font(.callout)
                        .foregroundStyle(.orange)
                    }
                } else {
                    Text("Looking for models on this Mac…")
                        .font(.title3)
                        .foregroundStyle(.secondary)
                }
                Text("Select a model to open it.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Spacer()
            }
            .frame(maxWidth: .infinity)
            .padding(24)
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

struct TierBadge: View {
    let tier: RunTier

    var body: some View {
        Text(label)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(color.opacity(0.18), in: Capsule())
            .foregroundStyle(color)
    }

    private var label: String {
        switch tier {
        case .native: "native"
        case .managed: "managed"
        case .recipeNeeded: "recipe"
        }
    }

    private var color: Color {
        switch tier {
        case .native: .green
        case .managed: .blue
        case .recipeNeeded: .gray
        }
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
        VStack(alignment: .leading, spacing: 16) {
            Text("Run \(record.name)?").font(.title3.weight(.semibold))
            Text(
                "Hedos will run this \(record.modality.rawValue) model via **\(record.runtime.id ?? "?")**."
            )
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
                Button("Confirm") {
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
        .padding(24)
        .frame(width: 380)
    }
}

struct RecipeNeededPane: View {
    let record: ModelRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text(record.name).font(.title2.weight(.semibold))
                TierBadge(tier: .recipeNeeded)
            }
            Text(
                "Hedos found this model but no built-in runtime can run it yet. It needs a runtime recipe — a small manifest that teaches Hedos how to execute it."
            )
            .foregroundStyle(.secondary)
            Text("Community recipes arrive with the runtime library in a later release.")
                .font(.caption)
                .foregroundStyle(.tertiary)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(24)
    }
}

struct ModelInfoPane: View {
    let record: ModelRecord

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(record.name).font(.title2.weight(.semibold))
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
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
                if let mb = record.footprintMB {
                    GridRow {
                        Text("Size").foregroundStyle(.secondary)
                        Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                    }
                }
                GridRow {
                    Text("State").foregroundStyle(.secondary)
                    Text(record.state.rawValue)
                }
            }
            .font(.callout)
            Text("Running this kind of model arrives in a later milestone.")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .padding(24)
    }
}
