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
        .clampedSheetFrame(
            width: Design.Sheet.installWidth, height: Design.Sheet.installHeight)
        .background(
            Button("") {
                if installs.stagedPlan != nil || installs.stagingID != nil {
                    installs.discardStagedPlan()
                } else if !installs.searchQuery
                    .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                {
                    installs.searchQuery = ""
                    installs.searchDebounced()
                } else {
                    onClose()
                }
            }
            .keyboardShortcut(shell.commandPaletteOpen ? nil : .cancelAction)
            .hidden()
            .accessibilityHidden(true))
        .task { await installs.load() }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            if let plan = installs.stagedPlan {
                QuietIconButton(glyph: "chevron.left") {
                    installs.discardStagedPlan()
                }
                .help("Back to browsing")
                .accessibilityLabel("Back")
                .accessibilityIdentifier("install-confirm-back")
                IconPlaque(size: 44) {
                    SourceMark(kind: installs.sourceKind(of: plan.provider), size: 24)
                        .foregroundStyle(Design.inkSoft)
                }
                VStack(alignment: .leading, spacing: Design.Space.xxs) {
                    Text(plan.displayName)
                        .font(Design.title)
                        .tracking(Design.tightTracking)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text(plan.reference)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .textSelection(.enabled)
                }
            } else {
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
            }
            Spacer()
            SheetCloseButton(usesCancelShortcut: false, action: onClose)
        }
        .animation(
            Design.snapMotion(reduceMotion: reduceMotion),
            value: installs.stagedPlan?.reference)
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
        let query = installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        return !query.isEmpty && InstallReference.ollamaDirectTag(from: query) == nil
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
            value: InstallReference.ollamaDirectTag(
                from: installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)))
    }

    @ViewBuilder
    private var directReferenceRow: some View {
        let query = installs.searchQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        if let tag = InstallReference.ollamaDirectTag(from: query) {
            Button {
                Task { await installs.stage(provider: .ollama, reference: tag) }
            } label: {
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: installs.sourceKind(of: .ollama), size: 14)
                    Text("Review \(tag)")
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
                    CategoryTabs(selection: $category)
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
