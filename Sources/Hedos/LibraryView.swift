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
        .safeAreaInset(edge: .top, alignment: .leading) { header }
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

    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            if model.isScanning && model.summary == nil {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text("Looking for models…").foregroundStyle(.secondary)
                }
                .font(.callout)
            } else if let summary = model.summary {
                Text(summary.headline)
                    .font(.callout.weight(.medium))
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(summary.duplicates, id: \.self) { group in
                    Label(
                        "\(group.names.joined(separator: " and ")) live in more than one place — \(DiscoverySummary.formatBytes(group.wastedBytes)) duplicated.",
                        systemImage: "externaldrive.badge.exclamationmark")
                    .foregroundStyle(.orange)
                    .font(.caption)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
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
            }
            if let mb = record.footprintMB {
                Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private var detail: some View {
        if let record = model.record(id: selectedID) {
            if record.capabilities.contains(.chat) {
                ChatView(record: record, kernel: model.kernel)
                    .id(record.id)
            } else {
                ModelInfoPane(record: record)
            }
        } else {
            ContentUnavailableView(
                "Select a model",
                systemImage: "square.stack.3d.up",
                description: Text("Chat-capable models open a conversation here."))
        }
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
