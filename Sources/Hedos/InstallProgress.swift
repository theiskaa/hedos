import HedosKernel
import SwiftUI

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
            if let status = installs.status(installID: install.id) {
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
        let downloaded = ByteFormat.string(progress.bytesDownloaded)
        guard let total = progress.totalBytes else { return downloaded }
        let suffix = progress.totalIsPartial ? "+" : ""
        return "\(downloaded) / \(ByteFormat.string(total))\(suffix)"
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
