import HedosKernel
import SwiftUI

@Observable
@MainActor
final class LibraryViewModel {
    private let kernel = Kernel()

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

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            if model.records.isEmpty && !model.isScanning {
                emptyState
            } else {
                shelf
            }
        }
        .frame(minWidth: 560, minHeight: 400)
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
        .task { await model.rescan() }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            if model.isScanning && model.summary == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Looking for models on this Mac…")
                        .foregroundStyle(.secondary)
                }
            } else if let summary = model.summary {
                Text(summary.headline)
                    .font(.title3.weight(.medium))
                ForEach(summary.duplicates, id: \.self) { group in
                    Label(
                        "\(group.names.joined(separator: " and ")) live in more than one place — \(DiscoverySummary.formatBytes(group.wastedBytes)) duplicated.",
                        systemImage: "externaldrive.badge.exclamationmark")
                    .foregroundStyle(.orange)
                    .font(.callout)
                }
                if let error = model.errorMessage {
                    Text(error).foregroundStyle(.red).font(.caption)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
    }

    private var shelf: some View {
        List {
            ForEach(model.groupedRecords, id: \.section) { group in
                Section(group.section) {
                    ForEach(group.records) { record in
                        row(record)
                    }
                }
            }
        }
        .listStyle(.inset)
    }

    private func row(_ record: ModelRecord) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(record.name)
                if let repo = record.source.repo, repo != record.name {
                    Text(repo).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if record.state == .missing {
                Text("missing")
                    .font(.caption.weight(.medium))
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(.orange.opacity(0.2), in: Capsule())
                    .foregroundStyle(.orange)
            }
            if let mb = record.footprintMB {
                Text(DiscoverySummary.formatBytes(Int64(mb) << 20))
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 80, alignment: .trailing)
            }
        }
        .padding(.vertical, 2)
    }

    private var emptyState: some View {
        VStack(spacing: 8) {
            Spacer()
            Text("No models found on this Mac yet.")
                .font(.title3)
            Text("Models from Ollama, the Hugging Face cache, LM Studio, and your Downloads folder appear here automatically.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .padding()
    }
}
