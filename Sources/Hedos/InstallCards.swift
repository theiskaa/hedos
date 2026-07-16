import HedosKernel
import SwiftUI

struct SearchResultCard: View {
    @Bindable var shell: ShellModel
    let hit: InstallSearchHit
    @State private var hovering = false

    private var installs: InstallModel { shell.installs }

    private var meta: String {
        var parts: [String] = []
        if let downloads = hit.downloads {
            parts.append("\(downloads.compactCount) downloads")
        }
        if let likes = hit.likes {
            parts.append("\(likes.compactCount) likes")
        }
        return parts.isEmpty ? hit.reference : parts.joined(separator: " · ")
    }

    private var installed: Bool {
        installs.installed(provider: hit.provider, reference: hit.reference)
    }

    private var stageable: Bool {
        !installed && installs.stagingID == nil
            && installs.activeInstall(provider: hit.provider, reference: hit.reference) == nil
    }

    private func stage() {
        Task {
            await installs.stage(provider: hit.provider, reference: hit.reference)
        }
    }

    private func installNow() {
        Task {
            await installs.install(provider: hit.provider, reference: hit.reference)
        }
    }

    var body: some View {
        Button(action: stage) {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: installs.sourceKind(of: hit.provider), size: 16)
                        .foregroundStyle(Design.inkSoft)
                    Spacer(minLength: 0)
                    if let updated = hit.updatedAt {
                        Text(
                            updated.formatted(.dateTime.month(.abbreviated).year()).uppercased()
                        )
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
                    if installed {
                        TintChip(text: "installed", glyph: "checkmark")
                    } else if installs.activeInstall(provider: hit.provider, reference: hit.reference) != nil {
                        ProgressView().controlSize(.small)
                    } else {
                        Button("Install", action: installNow)
                            .buttonStyle(QuietButtonStyle())
                            .disabled(!stageable)
                            .help("Start downloading right away")
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
            .contentShape(RoundedRectangle.soft(Design.Radius.tile))
        }
        .buttonStyle(.plain)
        .disabled(!stageable)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help(installed ? "\(hit.reference) is already on your shelf" : "Review \(hit.reference)")
        .accessibilityLabel(
            installed
                ? "\(hit.reference), already installed" : "Review \(hit.reference)")
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

struct CatalogInstallCard: View {
    @Bindable var shell: ShellModel
    let entry: InstallCatalogEntry
    @State private var hovering = false

    private var installs: InstallModel { shell.installs }

    private var verdict: FitVerdict? {
        entry.fit(totalMemoryBytes: HardwareProfile.current.totalMemoryBytes)?.verdict
    }

    private var installed: Bool {
        installs.installed(provider: entry.provider, reference: entry.reference)
    }

    private var stageable: Bool {
        !installed && installs.stagingID == nil
            && installs.activeInstall(provider: entry.provider, reference: entry.reference) == nil
            && installs.isAvailable(entry.provider)
    }

    private func stage() {
        Task { await installs.stage(entry: entry) }
    }

    private func installNow() {
        Task {
            await installs.install(provider: entry.provider, reference: entry.reference)
        }
    }

    var body: some View {
        Button(action: stage) {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                HStack(spacing: Design.Space.s) {
                    SourceMark(kind: installs.sourceKind(of: entry.provider), size: 16)
                        .foregroundStyle(Design.inkSoft)
                    Spacer(minLength: 0)
                    if let verdict, !installed {
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
                    action
                }
            }
            .padding(Design.Space.tile)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.tile))
            .overlay(
                RoundedRectangle.soft(Design.Radius.tile)
                    .strokeBorder(
                        hovering && stageable ? Design.accentEdge : Design.line,
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle.soft(Design.Radius.tile))
        }
        .buttonStyle(.plain)
        .disabled(!stageable)
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help(installed ? "\(entry.name) is already on your shelf" : "Review \(entry.name)")
        .accessibilityLabel(
            installed ? "\(entry.name), already installed" : "Review \(entry.name)")
    }

    @ViewBuilder
    private var action: some View {
        if installed {
            TintChip(text: "installed", glyph: "checkmark")
        } else if installs.activeInstall(provider: entry.provider, reference: entry.reference) != nil {
            ProgressView().controlSize(.small)
        } else if installs.isAvailable(entry.provider) {
            Button("Install", action: installNow)
                .buttonStyle(QuietButtonStyle())
                .disabled(!stageable)
                .help("Start downloading right away")
        } else {
            Text("unavailable")
                .font(Design.micro)
                .foregroundStyle(Design.inkFaint)
        }
    }
}
