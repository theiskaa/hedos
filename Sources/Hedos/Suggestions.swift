import AppKit
import HedosKernel
import SwiftUI

enum SuggestionCategories {
    static let ordered: [(category: InstallCategory, label: String)] = [
        (.chat, "Chat"),
        (.code, "Code"),
        (.voice, "Voice"),
        (.image, "Image"),
    ]

    static func label(_ verdict: FitVerdict) -> String {
        switch verdict {
        case .runsWell: "runs well"
        case .tightFit: "tight fit"
        case .tooLarge: "too large"
        }
    }
}

struct FirstRunDiscovery: View {
    @Bindable var shell: ShellModel
    @State private var category: InstallCategory = .chat

    private let hardware = HardwareProfile.current

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.pane) {
            invitation
            suggestions
        }
        .task { await shell.installs.load() }
    }

    private var invitation: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            Text("hedos never downloads behind your back. Point it at your disk and it finds the models you already have — or start from a recommendation tuned to your hardware.")
                .font(Design.readingBody)
                .lineSpacing(Design.readingLineSpacing)
                .foregroundStyle(Design.inkSoft)
                .frame(maxWidth: Design.Column.prose, alignment: .leading)
            Text(hardware.summary.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.accentText)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.s)
                .overlay(
                    RoundedRectangle.soft(Design.Radius.control)
                        .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            HStack(spacing: Design.Space.m) {
                Button("Scan this Mac") {
                    Task { await shell.library.rescan() }
                }
                .buttonStyle(InkButtonStyle())
                .disabled(shell.library.isScanning)
                Button("Choose a folder…") {
                    shell.openSettings(
                        at: SettingsDestination(section: .models, anchor: "models.folders"))
                }
                .buttonStyle(QuietButtonStyle())
            }
        }
    }

    private var visible: [InstallCatalogEntry] {
        InstallCatalog.entries.filter { $0.category == category }
    }

    private func verdict(_ entry: InstallCatalogEntry) -> FitVerdict? {
        entry.fit(totalMemoryBytes: hardware.totalMemoryBytes)?.verdict
    }

    private var recommendedID: InstallCatalogEntry.ID? {
        InstallCatalog.recommended(
            category: category, totalMemoryBytes: hardware.totalMemoryBytes
        ).last?.id
    }

    private var suggestions: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            HStack(alignment: .center) {
                MicroHeader(title: "Recommended for your Mac")
                Spacer(minLength: Design.Space.l)
                CategoryTabs(selection: $category)
            }
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 220), spacing: Design.Space.l)],
                spacing: Design.Space.l
            ) {
                ForEach(visible) { entry in
                    SuggestionCard(
                        shell: shell,
                        entry: entry,
                        verdict: verdict(entry) ?? .runsWell,
                        recommended: entry.id == recommendedID)
                }
            }
        }
    }

}

struct CategoryTabs: View {
    @Binding var selection: InstallCategory

    var body: some View {
        HStack(spacing: Design.Space.xs) {
            ForEach(SuggestionCategories.ordered, id: \.category) { item in
                let selected = item.category == selection
                Button {
                    selection = item.category
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
}

struct SuggestionCard: View {
    @Bindable var shell: ShellModel
    let entry: InstallCatalogEntry
    let verdict: FitVerdict
    let recommended: Bool
    @State private var hovering = false
    @State private var acted = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var installs: InstallModel { shell.installs }

    private var referenceURL: String {
        "https://huggingface.co/\(entry.reference)"
    }

    private var installable: Bool {
        installs.isAvailable(entry.provider)
    }

    private var activeInstall: ActiveInstall? {
        installs.activeInstall(provider: entry.provider, reference: entry.reference)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                SourceMark(kind: installs.sourceKind(of: entry.provider), size: 18)
                    .foregroundStyle(Design.inkSoft)
                Spacer(minLength: 0)
                Text((recommended ? "best for you" : SuggestionCategories.label(verdict)).uppercased())
                    .font(Design.label)
                    .tracking(Design.microTracking)
                    .foregroundStyle(verdict == .tightFit ? Design.heatText : Design.accentText)
            }
            Text(entry.name)
                .font(Design.title)
                .foregroundStyle(Design.ink)
                .lineLimit(1)
            Text(entry.blurb)
                .font(Design.readingBody)
                .lineSpacing(2)
                .foregroundStyle(Design.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            if let install = activeInstall {
                InstallProgressBar(
                    fraction: (installs.progress(installID: install.id) ?? install.progress)
                        .fraction)
                .transition(.arrive(from: .bottom, reduceMotion: reduceMotion))
            }
            if let failure = installs.failure(provider: entry.provider, reference: entry.reference) {
                Text(failure)
                    .font(Design.label)
                    .foregroundStyle(Design.heatText)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
                    .transition(.arrive(from: .bottom, reduceMotion: reduceMotion))
            }
            HStack {
                Text(String(format: "%g GB", entry.sizeGB))
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkFaint)
                Spacer(minLength: Design.Space.m)
                action
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, minHeight: 150, alignment: .topLeading)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
        .overlay(
            RoundedRectangle.soft(Design.Radius.card)
                .strokeBorder(
                    hovering
                        ? Design.accentEdge
                        : recommended ? Design.accentEdge : Design.line,
                    lineWidth: Design.hairlineWidth))
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .animation(Design.snapMotion(reduceMotion: reduceMotion), value: cardState)
        .help(helpText)
    }

    private var cardState: String {
        if installs.installed(provider: entry.provider, reference: entry.reference) {
            return "done"
        }
        if activeInstall != nil { return "downloading" }
        if let failure = installs.failure(provider: entry.provider, reference: entry.reference) { return "failed|\(failure)" }
        return "idle"
    }

    private var helpText: String {
        if installable {
            return "Download \(entry.reference) into its own store"
        }
        return entry.provider == .ollama
            ? "Copy: ollama pull \(entry.reference)" : referenceURL
    }

    @ViewBuilder
    private var action: some View {
        if installs.installed(provider: entry.provider, reference: entry.reference) {
            Text("On your shelf".uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.accentText)
        } else if activeInstall != nil {
            Text("Downloading…".uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
        } else if installable {
            cardButton("Install ▸") {
                Task { await installs.install(provider: entry.provider, reference: entry.reference) }
            }
        } else {
            cardButton(acted ? "Copied ✓" : entry.provider == .ollama ? "Get ▸" : "Open ▸") {
                get()
            }
        }
    }

    private func cardButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(Design.micro)
                .tracking(0.4)
                .foregroundStyle(recommended ? Design.onAccent : Design.ink)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.s)
                .background(
                    recommended ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.clear),
                    in: RoundedRectangle.soft(Design.Radius.control))
                .overlay(
                    RoundedRectangle.soft(Design.Radius.control)
                        .strokeBorder(
                            recommended ? Color.clear : Design.lineBright,
                            lineWidth: Design.hairlineWidth))
                .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
    }

    private func get() {
        switch entry.provider {
        case .ollama:
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString("ollama pull \(entry.reference)", forType: .string)
            acted = true
            Task {
                try? await Task.sleep(for: .seconds(1.6))
                acted = false
            }
        default:
            if let url = URL(string: referenceURL) {
                NSWorkspace.shared.open(url)
            }
        }
    }
}
