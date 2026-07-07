import AppKit
import HedosKernel
import SwiftUI

struct PromptBubble: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Design.body)
            .textSelection(.enabled)
            .padding(.horizontal, Design.Space.l)
            .padding(.vertical, Design.Space.m)
            .background(
                Design.accentWash, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.bubble)
                    .strokeBorder(Design.accentEdge, lineWidth: Design.hairlineWidth))
            .frame(maxWidth: Design.Bubble.promptMax, alignment: .trailing)
            .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

struct ResponseShell: ViewModifier {
    func body(content: Content) -> some View {
        content
            .padding(.horizontal, Design.Space.l)
            .padding(.vertical, Design.Space.m)
            .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.bubble)
                    .strokeBorder(Design.hairline, lineWidth: Design.hairlineWidth))
    }
}

extension View {
    func responseShell() -> some View {
        modifier(ResponseShell())
    }
}

struct VoiceBubble: View {
    let artifact: Artifact
    let clips: AudioClipController
    let onToggle: () -> Void

    private var isActive: Bool {
        clips.isActive(artifact.id)
    }

    private var isSounding: Bool {
        clips.isSounding(artifact.id)
    }

    var body: some View {
        HStack(spacing: Design.Space.chipX) {
            playButton
            WavePlayerBars(
                peaks: VoiceSurfaceModel.peaks(of: artifact),
                fraction: isActive ? clips.progress : 0,
                onSeek: { fraction in
                    if isActive {
                        clips.seek(to: fraction)
                    } else {
                        onToggle()
                    }
                })
            Text(timeText)
                .font(Design.data(10))
                .monospacedDigit()
                .foregroundStyle(Design.inkSoft)
                .lineLimit(1)
                .fixedSize()
            rateChip
        }
        .padding(.vertical, Design.Space.s)
        .padding(.leading, Design.Space.s)
        .padding(.trailing, Design.Space.l)
        .background(Design.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .frame(maxWidth: Design.Bubble.promptMax)
        .help(VoiceSurfaceModel.voiceName(of: artifact).map { "Voice: \($0)" } ?? "Narration")
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Narration, \(durationText)")
    }

    private var playButton: some View {
        Button(action: onToggle) {
            Image(systemName: isSounding ? "pause.fill" : "play.fill")
                .font(Design.caption.weight(.semibold))
                .foregroundStyle(Design.paper)
                .frame(width: 34, height: 34)
                .background(Design.ink, in: Circle())
                .contentShape(Circle())
        }
        .buttonStyle(PressDipStyle())
        .inkFocusRing(Circle())
        .help(isSounding ? "Pause" : isActive ? "Resume" : "Play")
        .accessibilityLabel(isSounding ? "Pause" : "Play")
    }

    private var rateChip: some View {
        Button {
            clips.cycleRate()
        } label: {
            Text(clips.rateLabel)
                .font(Design.data(10).weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(clips.rate == 1.0 ? Design.inkSoft : Design.accentText)
                .padding(.horizontal, Design.Space.m)
                .padding(.vertical, Design.Space.xs)
                .background(
                    clips.rate == 1.0
                        ? AnyShapeStyle(Design.inkWash) : AnyShapeStyle(Design.accentWash),
                    in: Capsule())
                .contentShape(Capsule())
        }
        .buttonStyle(PressDipStyle())
        .fixedSize()
        .help("Playback speed · click to cycle")
        .contextMenu {
            ForEach(AudioClipController.rates, id: \.self) { candidate in
                Button(String(format: "%g×", candidate)) {
                    clips.setRate(candidate)
                }
            }
        }
        .accessibilityLabel("Playback speed \(clips.rateLabel)")
    }

    private var timeText: String {
        "\(Self.clock(isActive ? clips.elapsed : 0)) / \(durationText)"
    }

    private var durationText: String {
        Self.clock(Double(max(1000, artifact.durationMs)) / 1000)
    }

    private static func clock(_ seconds: TimeInterval) -> String {
        let whole = max(0, Int(seconds.rounded()))
        return String(format: "%d:%02d", whole / 60, whole % 60)
    }
}

struct WavePlayerBars: View {
    let peaks: [Double]
    let fraction: Double
    var onSeek: ((Double) -> Void)? = nil

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                bars(AnyShapeStyle(Design.ink.opacity(0.2)))
                bars(AnyShapeStyle(Design.accent))
                    .mask(alignment: .leading) {
                        Rectangle()
                            .frame(width: max(0, geometry.size.width * fraction))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .animation(.linear(duration: 0.11), value: fraction)
                    }
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onEnded { value in
                        guard geometry.size.width > 0 else { return }
                        onSeek?(
                            min(max(value.location.x / geometry.size.width, 0), 1))
                    })
        }
        .frame(height: 26)
        .accessibilityHidden(true)
    }

    private func bars(_ style: AnyShapeStyle) -> some View {
        let normalized = displayPeaks
        return HStack(alignment: .center, spacing: 1.5) {
            ForEach(Array(normalized.enumerated()), id: \.offset) { _, level in
                Capsule()
                    .fill(style)
                    .frame(maxWidth: .infinity)
                    .frame(height: (0.14 + 0.86 * level) * 26)
            }
        }
    }

    private var displayPeaks: [Double] {
        let source = peaks.isEmpty ? Array(repeating: 0.4, count: 80) : peaks
        let resampled = Self.resample(source, to: 80)
        let low = resampled.min() ?? 0
        let range = max((resampled.max() ?? 1) - low, 0.001)
        return resampled.map { ($0 - low) / range }
    }

    private static func resample(_ source: [Double], to target: Int) -> [Double] {
        guard source.count != target, !source.isEmpty else { return source }
        if source.count > target {
            let bucket = Double(source.count) / Double(target)
            return (0..<target).map { index in
                let start = Int(Double(index) * bucket)
                let end = min(Int(Double(index + 1) * bucket), source.count)
                let slice = source[start..<max(end, start + 1)]
                return slice.max() ?? 0
            }
        }
        return (0..<target).map { index in
            let position = Double(index) * Double(source.count - 1) / Double(target - 1)
            let low = Int(position)
            let high = min(low + 1, source.count - 1)
            let t = position - Double(low)
            return source[low] * (1 - t) + source[high] * t
        }
    }
}

struct ImageBubble: View {
    let image: NSImage?
    let caption: String?
    let isLoading: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            Group {
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .interpolation(.high)
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: Design.Radius.card))
                } else if isLoading {
                    SkeletonPulse()
                        .aspectRatio(1, contentMode: .fit)
                } else {
                    RoundedRectangle(cornerRadius: Design.Radius.card)
                        .fill(Design.cardFill)
                        .aspectRatio(1, contentMode: .fit)
                }
            }
            .frame(maxWidth: Design.Bubble.imageMax, maxHeight: Design.Bubble.imageMax)
            if let caption, !caption.isEmpty {
                Text(caption)
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: Design.Bubble.imageMax)
            }
        }
        .padding(Design.Space.m)
        .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
        .overlay(
            RoundedRectangle(cornerRadius: Design.Radius.bubble)
                .strokeBorder(Design.hairline, lineWidth: Design.hairlineWidth))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(
            isLoading
                ? "Image generating"
                : "Generated image" + (caption.map { ", \($0)" } ?? ""))
    }
}
