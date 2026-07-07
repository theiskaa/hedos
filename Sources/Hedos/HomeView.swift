import HedosKernel
import SwiftUI

struct HomePane: View {
    @Bindable var shell: ShellModel

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Home") {
                if shell.library.isScanning {
                    ProgressView()
                        .controlSize(.small)
                }
                QuietIconButton(glyph: "arrow.clockwise") {
                    Task { await shell.library.rescan() }
                }
                .disabled(shell.library.isScanning)
                .help("Scan the machine again")
                .accessibilityLabel("Rescan")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    hero
                        .padding(.top, Design.Space.pane)
                    valueStrip
                        .padding(.top, Design.Space.pane + Design.Space.m)
                    HStack(alignment: .top, spacing: Design.Space.xl) {
                        modelsCard
                        galleryCard
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, Design.Space.pane + Design.Space.m)
                }
                .padding(.horizontal, Design.Space.gutter + Design.Space.m)
                .padding(.bottom, Design.Space.pane)
                .frame(maxWidth: 780, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .task {
            if shell.library.summary == nil {
                await shell.library.rescan()
            }
            await shell.images.load()
        }
    }

    private var summary: DiscoverySummary? {
        shell.library.summary
    }

    private var hero: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Discovered · Local · Yours".uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
                .padding(.bottom, Design.Space.xxl)
            headline
                .padding(.bottom, Design.Space.xl)
            Text(supportingLine)
                .font(Design.heroBody)
                .foregroundStyle(Design.inkSoft)
                .lineSpacing(5)
                .frame(maxWidth: 520, alignment: .leading)
                .padding(.bottom, Design.Space.gutter)
            HStack(spacing: Design.Space.xxl) {
                Button("New Chat") {
                    shell.newChat()
                }
                .buttonStyle(InkButtonStyle())
                .disabled(Launcher.defaultChatModel(in: shell.library.records) == nil)
                Button {
                    shell.setMode(.library)
                } label: {
                    Text("Browse models →")
                        .font(Design.caption.weight(.medium))
                        .foregroundStyle(browseHovering ? Design.ink : Design.inkSoft)
                        .animation(Design.wash, value: browseHovering)
                }
                .buttonStyle(.plain)
                .onHover { browseHovering = $0 }
                .accessibilityLabel("Browse models")
            }
            .padding(.bottom, Design.Space.xl)
            Text("Images ⌘2 · Voice ⌘3 · Models ⌘4".uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
        }
    }

    @State private var browseHovering = false

    @ViewBuilder
    private var headline: some View {
        if let failure = shell.library.errorMessage {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                Text("The scan hit a problem.")
                    .font(Design.hero)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                Text(failure)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
            }
        } else if let summary, summary.totalCount > 0 {
            HStack(alignment: .firstTextBaseline, spacing: 0) {
                Text("\(summary.totalCount) models")
                    .font(Design.hero)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                    .padding(.horizontal, Design.Space.xs)
                    .background(
                        RoundedRectangle(cornerRadius: Design.Radius.card)
                            .fill(Design.ink.opacity(0.08))
                            .padding(.vertical, Design.Space.xxs))
                Text(" live on this Mac.")
                    .font(Design.hero)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
            }
            .padding(.leading, -Design.Space.xs)
        } else if summary != nil {
            Text("No models on this Mac yet.")
                .font(Design.hero)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
        } else {
            Text("Looking around this Mac…")
                .font(Design.hero)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
        }
    }

    private var supportingLine: String {
        guard let summary else {
            return "The scan checks Ollama, the Hugging Face cache, LM Studio, and your watched folders."
        }
        guard summary.totalCount > 0 else {
            return "Install through Ollama or point Hedos at a folder — the next scan finds everything, nothing gets moved or copied."
        }
        var parts: [String] = []
        if let stat = summary.perKind[.ollama], stat.count > 0 {
            parts.append("\(stat.count) in Ollama")
        }
        if let stat = summary.perKind[.huggingfaceCache], stat.count > 0 {
            parts.append("\(stat.count) in the Hugging Face cache")
        }
        if let stat = summary.perKind[.lmStudio], stat.count > 0 {
            parts.append("\(stat.count) in LM Studio")
        }
        let loose = (summary.perKind[.file]?.count ?? 0) + (summary.perKind[.folder]?.count ?? 0)
        if loose > 0 {
            parts.append("\(loose) loose \(loose == 1 ? "file" : "files")")
        }
        let breakdown = parts.joined(separator: ", ")
        let total = DiscoverySummary.formatBytes(summary.totalBytes)
        return "\(breakdown) — \(total) on disk, every one of them local and private."
    }

    private var valueStrip: some View {
        VStack(alignment: .leading, spacing: 0) {
            Rectangle()
                .fill(Design.line)
                .frame(height: Design.hairlineWidth)
                .padding(.bottom, Design.Space.gutter)
            HStack(alignment: .top, spacing: 0) {
                ForEach(Array(stripColumns.enumerated()), id: \.offset) { index, column in
                    VStack(alignment: .leading, spacing: Design.Space.s) {
                        Text(column.eyebrow.uppercased())
                            .font(Design.micro)
                            .tracking(Design.microTracking)
                            .foregroundStyle(Design.inkFaint)
                        Text(column.claim)
                            .font(Design.body.weight(.semibold))
                            .foregroundStyle(Design.ink)
                        Text(column.caption)
                            .font(Design.label)
                            .foregroundStyle(Design.inkSoft)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.trailing, Design.Space.xl)
                    if index < stripColumns.count - 1 {
                        Rectangle()
                            .fill(Design.line)
                            .frame(width: Design.hairlineWidth)
                            .padding(.trailing, Design.Space.xl)
                    }
                }
            }
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var stripColumns: [(eyebrow: String, claim: String, caption: String)] {
        guard let summary, summary.totalCount > 0 else {
            return [
                ("Stores", "Four places, one scan", "Ollama, Hugging Face, LM Studio, folders."),
                ("Weights", "Nothing moves", "Records point at where your tools put them."),
                ("Privacy", "Nothing leaves", "Every model runs on this machine."),
            ]
        }
        var columns: [(String, String, String)] = []
        let stores: [(SourceKind, String)] = [
            (.ollama, "Ollama"), (.huggingfaceCache, "Hugging Face"), (.lmStudio, "LM Studio"),
        ]
        for (kind, name) in stores {
            if let stat = summary.perKind[kind], stat.count > 0 {
                columns.append(
                    (
                        name,
                        "\(stat.count) \(stat.count == 1 ? "model" : "models")",
                        "\(DiscoverySummary.formatBytes(stat.bytes)) on disk"
                    ))
            }
        }
        let loose = (summary.perKind[.file]?.count ?? 0) + (summary.perKind[.folder]?.count ?? 0)
        if loose > 0 {
            let bytes = (summary.perKind[.file]?.bytes ?? 0) + (summary.perKind[.folder]?.bytes ?? 0)
            columns.append(
                (
                    "Loose",
                    "\(loose) \(loose == 1 ? "file" : "files")",
                    "\(DiscoverySummary.formatBytes(bytes)) on disk"
                ))
        }
        if !summary.duplicates.isEmpty {
            let wasted = summary.duplicates.reduce(Int64(0)) { $0 + $1.wastedBytes }
            columns.append(
                (
                    "Duplicates",
                    "\(summary.duplicates.count) shared",
                    "\(DiscoverySummary.formatBytes(wasted)) twice on disk"
                ))
        }
        return Array(columns.prefix(4))
    }

    private var homeModels: [ModelRecord] {
        let ranked = shell.library.records.sorted { first, second in
            if (first.state == .ready) != (second.state == .ready) {
                return first.state == .ready
            }
            return first.displayName.localizedCaseInsensitiveCompare(second.displayName)
                == .orderedAscending
        }
        let managed = ranked.filter { $0.runtime.tier == .managed }
        let pool = managed.isEmpty ? ranked : managed
        return Array(pool.prefix(4))
    }

    private var modelsCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            MicroHeader(title: "Models")
            LazyVGrid(
                columns: [
                    GridItem(.adaptive(minimum: 140), spacing: Design.Space.m, alignment: .top)
                ],
                spacing: Design.Space.m
            ) {
                ForEach(homeModels) { record in
                    HomeModelCard(record: record) {
                        if record.runtime.tier == .recipeNeeded {
                            shell.selectLibrary(record.id)
                            shell.setMode(.library)
                        } else {
                            shell.launch(record)
                        }
                    }
                }
            }
            if shell.library.records.count > homeModels.count {
                Button("All models") {
                    shell.setMode(.library)
                }
                .buttonStyle(QuietButtonStyle())
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard()
        .shadow(color: Design.shadowColor.opacity(0.06), radius: 14, x: 0, y: 5)
    }

    private var homeArtifacts: [Artifact] {
        Array(shell.images.arranged.prefix(2))
    }

    private var galleryCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            MicroHeader(title: "Images")
            if homeArtifacts.isEmpty {
                Text("Nothing generated yet — an image model renders here.")
                    .font(Design.caption)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.m)
            } else {
                LazyVGrid(
                    columns: [
                        GridItem(.adaptive(minimum: 110), spacing: Design.Space.m, alignment: .top)
                    ],
                    spacing: Design.Space.m
                ) {
                    ForEach(homeArtifacts, id: \.id) { artifact in
                        HomeGalleryCell(artifact: artifact, shell: shell)
                    }
                }
                if shell.images.arranged.count > homeArtifacts.count {
                    Button("All images") {
                        shell.setMode(.images)
                    }
                    .buttonStyle(QuietButtonStyle())
                }
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard()
        .shadow(color: Design.shadowColor.opacity(0.06), radius: 14, x: 0, y: 5)
    }
}

private struct HomeGalleryCell: View {
    let artifact: Artifact
    let shell: ShellModel
    @State private var hovering = false

    var body: some View {
        Button {
            shell.showArtifact(artifact.id)
        } label: {
            Color.clear
                .aspectRatio(1, contentMode: .fit)
                .frame(maxWidth: .infinity)
                .overlay {
                    if let image = shell.images.thumbnail(artifact) {
                        Image(nsImage: image)
                            .resizable()
                            .scaledToFill()
                    } else {
                        Rectangle().fill(Design.cardFill)
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: Design.Radius.card))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.card)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.card))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .lifts(hovering: hovering)
        .task(id: artifact.id) {
            await shell.images.loadThumbnail(artifact)
        }
        .accessibilityLabel(Provenance.prompt(of: artifact.params) ?? "Generated image")
    }
}

private struct HomeModelCard: View {
    let record: ModelRecord
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                SourceMark(kind: record.source.kind, size: 16)
                VStack(alignment: .leading, spacing: Design.Space.xxs) {
                    Text(record.displayName)
                        .font(Design.caption.weight(.medium))
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(subtitle.uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                        .lineLimit(1)
                }
            }
            .padding(Design.Space.l)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Design.paper, in: RoundedRectangle(cornerRadius: Design.Radius.tile))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.tile)
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.tile))
            .lifts(hovering: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(record.displayName)
    }

    private var subtitle: String {
        if record.runtime.tier == .recipeNeeded {
            return "Needs recipe"
        }
        guard let footprintMB = record.footprintMB else { return "Ready" }
        return DiscoverySummary.formatBytes(Int64(footprintMB) << 20)
    }
}
