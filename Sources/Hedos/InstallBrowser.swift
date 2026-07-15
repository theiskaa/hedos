import HedosKernel
import SwiftUI

struct InstallBrowser: View {
    @Bindable var shell: ShellModel
    let onClose: () -> Void

    @State private var category: InstallCategory = .chat
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var installs: InstallModel { shell.installs }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ZStack {
                if let plan = installs.stagedPlan {
                    InstallConfirmPage(shell: shell, plan: plan)
                        .transition(.arrive(from: .trailing, reduceMotion: reduceMotion))
                } else {
                    browsePage
                        .transition(.arrive(from: .leading, reduceMotion: reduceMotion))
                }
            }
            .animation(
                Design.motion(reduceMotion: reduceMotion),
                value: installs.stagedPlan != nil)
        }
        .frame(width: Design.Sheet.installWidth, height: Design.Sheet.installHeight)
        .task { await installs.load() }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: "arrow.down.circle")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text("Install models")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text("Pulled into each platform's own store — nothing hidden, nothing moved.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var browsePage: some View {
        VStack(alignment: .leading, spacing: 0) {
            searchBar
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.xl)
                .padding(.bottom, Design.Space.l)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    if !installs.active.isEmpty {
                        downloadingNow
                            .transition(.arrive(from: .top, reduceMotion: reduceMotion))
                    }
                    if !installs.failures.isEmpty {
                        failuresSection
                            .transition(.arrive(from: .top, reduceMotion: reduceMotion))
                    }
                    catalogSection
                    providerStrip
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.xs)
                .padding(.bottom, Design.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(
                    Design.motion(reduceMotion: reduceMotion),
                    value: installs.active.isEmpty)
                .animation(
                    Design.motion(reduceMotion: reduceMotion),
                    value: installs.failures.keys.sorted())
            }
        }
    }

    private var searchIsShowing: Bool {
        !installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var searchBar: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            InkSearchField(
                placeholder: "Search Hugging Face, or paste gemma3:4b / org/repo",
                query: Binding(
                    get: { installs.searchQuery },
                    set: { installs.searchQuery = $0 }),
                fill: Design.panel)
            .onChange(of: installs.searchQuery) { installs.searchDebounced() }
            directReferenceRow
                .transition(.arrive(from: .top, reduceMotion: reduceMotion))
        }
        .animation(
            Design.snapMotion(reduceMotion: reduceMotion),
            value: InstallBrowser.directProvider(
                for: installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)))
    }

    @ViewBuilder
    private var directReferenceRow: some View {
        let query = installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        if let provider = InstallBrowser.directProvider(for: query) {
            Button {
                Task { await installs.stage(provider: provider, reference: query) }
            } label: {
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: installs.sourceKind(of: provider), size: 14)
                    Text("Review \(query)")
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Image(systemName: "arrow.right")
                        .font(Design.glyphSmall)
                    if installs.stagingID != nil {
                        ProgressView().controlSize(.small)
                    }
                }
                .font(Design.caption)
                .foregroundStyle(Design.accentText)
                .contentShape(Rectangle())
            }
            .buttonStyle(PressDipStyle())
            .disabled(installs.stagingID != nil)
        }
    }

    static func directProvider(for query: String) -> InstallProviderID? {
        guard !query.isEmpty, !query.contains(" "), !query.contains("://") else { return nil }
        if query.split(separator: "/").count == 2, !query.contains(":") {
            return .huggingface
        }
        if query.contains(":") {
            return .ollama
        }
        return nil
    }

    private var downloadingNow: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Downloading now")
            VStack(spacing: Design.Space.s) {
                ForEach(installs.active) { install in
                    ActiveInstallRow(installs: installs, install: install)
                }
            }
        }
    }

    private var failuresSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            ForEach(installs.failures.sorted(by: { $0.key < $1.key }), id: \.key) { reference, message in
                HStack(alignment: .firstTextBaseline, spacing: Design.Space.m) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.heatText)
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        Text(reference)
                            .font(Design.caption.weight(.medium))
                            .foregroundStyle(Design.ink)
                        Text(message)
                            .font(Design.label)
                            .foregroundStyle(Design.inkSoft)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: Design.Space.m)
                    Button {
                        installs.dismissFailure(reference: reference)
                    } label: {
                        Image(systemName: "xmark")
                            .font(Design.glyphSmall)
                            .foregroundStyle(Design.inkFaint)
                            .frame(width: 20, height: 20)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(PressDipStyle())
                    .accessibilityLabel("Dismiss failure for \(reference)")
                }
                .padding(Design.Space.m)
                .surfaceCard(radius: Design.Radius.tile)
            }
        }
    }

    private var catalogSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack(alignment: .center) {
                MicroHeader(
                    title: searchIsShowing ? "Hugging Face results" : "Recommended for your Mac")
                Spacer(minLength: Design.Space.l)
                if !searchIsShowing {
                    categoryTabs
                }
            }
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 210), spacing: Design.Space.m)],
                spacing: Design.Space.m
            ) {
                if installs.searching {
                    ForEach(0..<6, id: \.self) { _ in
                        ShimmerInstallCard()
                    }
                } else if searchIsShowing {
                    ForEach(Array(installs.searchHits.prefix(9).enumerated()), id: \.element.id) {
                        index, hit in
                        SearchResultCard(shell: shell, hit: hit)
                            .staggeredArrival(index)
                    }
                } else {
                    ForEach(Array(visibleEntries.enumerated()), id: \.element.id) { index, entry in
                        CatalogInstallCard(shell: shell, entry: entry)
                            .staggeredArrival(index)
                    }
                }
            }
            .animation(Design.wash, value: installs.searching)
            .animation(Design.wash, value: searchIsShowing)
            if searchIsShowing, !installs.searching {
                if let error = installs.searchError {
                    Text(error)
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                } else if installs.searchHits.isEmpty {
                    Text("Nothing matched. Try another name, or paste an exact org/repo.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            if let error = installs.stageError {
                Text(error)
                    .font(Design.label)
                    .foregroundStyle(Design.heatText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var visibleEntries: [InstallCatalogEntry] {
        installs.catalog.filter { $0.category == category }
    }

    private var categoryTabs: some View {
        HStack(spacing: Design.Space.xs) {
            ForEach(SuggestionCategories.ordered, id: \.category) { item in
                let selected = item.category == category
                Button {
                    category = item.category
                } label: {
                    Text(item.label)
                        .font(Design.micro)
                        .tracking(0.4)
                        .foregroundStyle(selected ? Design.onAccent : Design.inkSoft)
                        .padding(.horizontal, Design.Space.chipX)
                        .padding(.vertical, Design.Space.xs + 1)
                        .background(
                            selected ? AnyShapeStyle(Design.accent) : AnyShapeStyle(Design.panel),
                            in: RoundedRectangle.soft(Design.Radius.control))
                        .overlay(
                            RoundedRectangle.soft(Design.Radius.control)
                                .strokeBorder(
                                    selected ? Color.clear : Design.line,
                                    lineWidth: Design.hairlineWidth))
                        .contentShape(RoundedRectangle.soft(Design.Radius.control))
                }
                .buttonStyle(PressDipStyle())
                .accessibilityLabel(item.label)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
    }

    @ViewBuilder
    private var providerStrip: some View {
        let unavailable = installs.providers.filter { $0.availability != .ready }
        if !unavailable.isEmpty {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                ForEach(unavailable) { status in
                    if case .unavailable(let hint) = status.availability {
                        HStack(spacing: Design.Space.s) {
                            SourceMark(kind: status.sourceKind, size: 14)
                                .foregroundStyle(Design.inkFaint)
                            Text("\(status.displayName): \(hint)")
                                .font(Design.label)
                                .foregroundStyle(Design.inkSoft)
                        }
                    }
                }
            }
        }
    }
}

struct ActiveInstallRow: View {
    let installs: InstallModel
    let install: ActiveInstall

    var body: some View {
        let progress = installs.progress(installID: install.id) ?? install.progress
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack(spacing: Design.Space.m) {
                SourceMark(kind: installs.sourceKind(of: install.provider), size: 16)
                    .foregroundStyle(Design.inkSoft)
                Text(install.displayName)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(Design.ink)
                    .lineLimit(1)
                Spacer(minLength: Design.Space.m)
                Text(Self.byteLabel(progress))
                    .font(Design.data(11))
                    .monospacedDigit()
                    .foregroundStyle(Design.inkFaint)
                Button {
                    Task { await installs.cancel(installID: install.id) }
                } label: {
                    Image(systemName: "xmark")
                        .font(Design.glyphSmall.weight(.bold))
                        .foregroundStyle(Design.inkSoft)
                        .frame(width: 20, height: 20)
                        .background(Design.surface, in: Circle())
                        .overlay(
                            Circle().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                        .contentShape(Circle())
                }
                .buttonStyle(PressDipStyle())
                .help("Cancel this download")
                .accessibilityLabel("Cancel installing \(install.displayName)")
            }
            InstallProgressBar(fraction: progress.fraction)
            if let status = installs.statusByID[install.id] {
                Text(status)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
            } else if let file = progress.currentFile {
                Text(file)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        }
        .padding(Design.Space.m)
        .surfaceCard(radius: Design.Radius.tile)
    }

    static func byteLabel(_ progress: InstallProgress) -> String {
        let downloaded = DiscoverySummary.formatBytes(progress.bytesDownloaded)
        guard let total = progress.totalBytes else { return downloaded }
        return "\(downloaded) / \(DiscoverySummary.formatBytes(total))"
    }
}

struct InstallProgressBar: View {
    let fraction: Double?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Design.line)
                if let fraction {
                    Capsule()
                        .fill(Design.accent)
                        .frame(width: max(geometry.size.width * fraction, 3))
                        .animation(Design.motion(reduceMotion: reduceMotion), value: fraction)
                } else {
                    Capsule()
                        .fill(Design.accentWash)
                        .overlay(
                            SheenBand(tint: Design.accent, opacity: 0.9)
                                .clipShape(Capsule()))
                }
            }
        }
        .frame(height: 3)
        .accessibilityHidden(true)
    }
}

struct SearchResultCard: View {
    @Bindable var shell: ShellModel
    let hit: InstallSearchHit
    @State private var hovering = false

    private var installs: InstallModel { shell.installs }

    private var meta: String {
        var parts: [String] = []
        if let downloads = hit.downloads {
            parts.append("\(Self.compact(downloads)) downloads")
        }
        if let likes = hit.likes {
            parts.append("\(Self.compact(likes)) likes")
        }
        return parts.isEmpty ? hit.reference : parts.joined(separator: " · ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack(spacing: Design.Space.s) {
                SourceMark(kind: installs.sourceKind(of: hit.provider), size: 16)
                    .foregroundStyle(Design.inkSoft)
                Spacer(minLength: 0)
                if let updated = hit.updatedAt {
                    Text(updated.formatted(.dateTime.month(.abbreviated).year()).uppercased())
                        .font(Design.label)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            Text(hit.name)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
            Text(meta)
                .font(Design.label)
                .foregroundStyle(Design.inkSoft)
                .lineLimit(2, reservesSpace: true)
            HStack {
                Text(hit.reference.split(separator: "/").first.map(String.init) ?? "")
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: Design.Space.s)
                if installs.activeInstall(reference: hit.reference) != nil {
                    ProgressView().controlSize(.small)
                } else {
                    Button("Install") {
                        Task {
                            await installs.stage(
                                provider: hit.provider, reference: hit.reference)
                        }
                    }
                    .buttonStyle(QuietButtonStyle())
                    .disabled(installs.stagingID != nil)
                }
            }
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
        .overlay(
            RoundedRectangle.soft(Design.Radius.tile)
                .strokeBorder(
                    hovering ? Design.accentEdge : Design.line,
                    lineWidth: Design.hairlineWidth))
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help(hit.reference)
    }

    static func compact(_ value: Int) -> String {
        switch value {
        case 1_000_000...:
            String(format: "%.1fM", Double(value) / 1_000_000)
        case 1_000...:
            String(format: "%.1fk", Double(value) / 1_000)
        default:
            String(value)
        }
    }
}

struct ShimmerInstallCard: View {
    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                SkeletonPulse(radius: Design.Radius.control)
                    .frame(width: 16, height: 16)
                Spacer(minLength: 0)
                SkeletonPulse(radius: Design.Radius.control)
                    .frame(width: 52, height: 9)
            }
            SkeletonPulse(radius: Design.Radius.control)
                .frame(width: 120, height: 12)
            SkeletonPulse(radius: Design.Radius.control)
                .frame(maxWidth: .infinity)
                .frame(height: 9)
            SkeletonPulse(radius: Design.Radius.control)
                .frame(width: 140, height: 9)
            HStack {
                SkeletonPulse(radius: Design.Radius.control)
                    .frame(width: 44, height: 10)
                Spacer(minLength: Design.Space.s)
                SkeletonPulse(radius: Design.Radius.control)
                    .frame(width: 56, height: 20)
            }
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
        .overlay(
            RoundedRectangle.soft(Design.Radius.tile)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .accessibilityHidden(true)
    }
}

struct InstallConfirmPage: View {
    @Bindable var shell: ShellModel
    let plan: InstallPlan

    @State private var beginning = false

    private var installs: InstallModel { shell.installs }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    identity
                    VStack(alignment: .leading, spacing: Design.Space.m) {
                        confirmRow("shippingbox", "From", providerName)
                        confirmRow("folder", "Lands in", plan.destination)
                        confirmRow("scalemass", "Download size", sizeLine)
                        if !plan.files.isEmpty {
                            confirmRow("doc.on.doc", "Files", filesLine)
                        }
                        if plan.requiresAuth {
                            confirmRow(
                                "lock", "Gated model",
                                "Needs a Hugging Face access token: sign in with `huggingface-cli login` or set HF_TOKEN, then review again.",
                                heat: true)
                        }
                    }
                    .padding(Design.Space.tile)
                    .surfaceCard(radius: Design.Radius.tile)
                    if let error = installs.stageError {
                        Text(error)
                            .font(Design.label)
                            .foregroundStyle(Design.heatText)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            HStack {
                Button {
                    installs.discardStagedPlan()
                } label: {
                    HStack(spacing: Design.Space.xs) {
                        Image(systemName: "chevron.left")
                            .font(Design.glyphSmall)
                        Text("Back")
                    }
                }
                .buttonStyle(QuietButtonStyle())
                .keyboardShortcut(.cancelAction)
                Spacer()
                Button(beginning ? "Starting…" : "Install") {
                    beginning = true
                    Task {
                        _ = await installs.confirm(plan)
                        beginning = false
                    }
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(beginning || plan.requiresAuth)
                .accessibilityIdentifier("model-install-confirm")
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.vertical, Design.Space.l)
        }
    }

    private var identity: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                SourceMark(kind: installs.sourceKind(of: plan.provider), size: 24)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(plan.displayName)
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                    .foregroundStyle(Design.ink)
                Text(plan.reference)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .textSelection(.enabled)
            }
        }
    }

    private var providerName: String {
        installs.providers.first { $0.id == plan.provider }?.displayName
            ?? plan.provider.rawValue
    }

    private var sizeLine: String {
        if let total = plan.totalBytes {
            if let remaining = plan.remainingBytes, remaining < total {
                return "\(DiscoverySummary.formatBytes(total)) — \(DiscoverySummary.formatBytes(total - remaining)) already here, \(DiscoverySummary.formatBytes(remaining)) to go"
            }
            return DiscoverySummary.formatBytes(total)
        }
        return plan.provider == .ollama
            ? "Reported by Ollama once the pull starts."
            : "Unknown until the download starts."
    }

    private var filesLine: String {
        let chosen = plan.files.map(\.path).sorted()
        let weights = chosen.filter { path in
            ["safetensors", "gguf", "bin", "ckpt", "pt", "pth"].contains(
                (path as NSString).pathExtension.lowercased())
        }
        let headline = "\(chosen.count) file\(chosen.count == 1 ? "" : "s")"
        guard let first = weights.first else { return headline }
        return "\(headline), including \(first)"
    }

    private func confirmRow(
        _ glyph: String, _ title: String, _ detail: String, heat: Bool = false
    ) -> some View {
        HStack(alignment: .top, spacing: Design.Space.m) {
            Image(systemName: glyph)
                .font(Design.glyphInline)
                .foregroundStyle(heat ? Design.heatText : Design.inkSoft)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(title)
                    .font(Design.caption.weight(.medium))
                    .foregroundStyle(heat ? Design.heatText : Design.ink)
                Text(detail)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineSpacing(1.5)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
        }
    }
}

struct CatalogInstallCard: View {
    @Bindable var shell: ShellModel
    let entry: InstallCatalogEntry
    @State private var hovering = false

    private var installs: InstallModel { shell.installs }

    private var verdict: FitVerdict? {
        entry.fit(totalMemoryBytes: ProcessInfo.processInfo.physicalMemory)?.verdict
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack(spacing: Design.Space.s) {
                SourceMark(kind: installs.sourceKind(of: entry.provider), size: 16)
                    .foregroundStyle(Design.inkSoft)
                Spacer(minLength: 0)
                if let verdict {
                    Text(SuggestionCategories.label(verdict).uppercased())
                        .font(Design.label)
                        .tracking(Design.microTracking)
                        .foregroundStyle(
                            verdict == .tightFit ? Design.heatText : Design.accentText)
                }
            }
            Text(entry.name)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.ink)
                .lineLimit(1)
            Text(entry.blurb)
                .font(Design.label)
                .foregroundStyle(Design.inkSoft)
                .lineLimit(2, reservesSpace: true)
            HStack {
                Text(String(format: "%g GB", entry.sizeGB))
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkFaint)
                Spacer(minLength: Design.Space.s)
                actionButton
            }
        }
        .padding(Design.Space.tile)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
        .overlay(
            RoundedRectangle.soft(Design.Radius.tile)
                .strokeBorder(
                    hovering ? Design.accentEdge : Design.line,
                    lineWidth: Design.hairlineWidth))
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
    }

    @ViewBuilder
    private var actionButton: some View {
        if installs.completed.contains(entry.reference) {
            Text("On your shelf")
                .font(Design.micro)
                .foregroundStyle(Design.accentText)
        } else if installs.activeInstall(reference: entry.reference) != nil {
            ProgressView().controlSize(.small)
        } else if installs.isAvailable(entry.provider) {
            Button("Install") {
                Task { await installs.stage(entry: entry) }
            }
            .buttonStyle(QuietButtonStyle())
            .disabled(installs.stagingID != nil)
        } else {
            Text("unavailable")
                .font(Design.micro)
                .foregroundStyle(Design.inkFaint)
        }
    }
}
