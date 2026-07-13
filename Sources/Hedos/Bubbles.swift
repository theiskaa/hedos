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
                Design.accentWash, in: RoundedRectangle.soft(Design.Radius.bubble))
            .overlay(
                RoundedRectangle.soft(Design.Radius.bubble)
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
            .background(Design.cardFill, in: RoundedRectangle.soft(Design.Radius.bubble))
            .hairlineBorder(RoundedRectangle.soft(Design.Radius.bubble))
    }
}

extension View {
    func responseShell() -> some View {
        modifier(ResponseShell())
    }
}

struct VoiceBubble: View {
    let artifact: Artifact
    let session: AudioSession
    let onToggle: () -> Void
    @State private var displayPeaks: [Double] = []

    private var isActive: Bool {
        session.isActive(artifact.id)
    }

    private var isSounding: Bool {
        session.isSounding(artifact.id)
    }

    var body: some View {
        HStack(spacing: Design.Space.chipX) {
            playButton
            WavePlayerBars(
                peaks: displayPeaks,
                fraction: isActive ? session.progress : 0,
                onSeek: { fraction in
                    if isActive {
                        session.seek(to: fraction)
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
        }
        .padding(.vertical, Design.Space.m)
        .padding(.horizontal, Design.Space.l)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.artifact))
        .hairlineBorder(RoundedRectangle.soft(Design.Radius.artifact))
        .frame(maxWidth: 340)
        .help(SpeechArtifact.voiceName(of: artifact).map { "Voice: \($0)" } ?? "Narration")
        .contextMenu {
            ForEach(AudioSession.rates, id: \.self) { candidate in
                Button(String(format: "%g× speed", candidate)) {
                    session.setRate(candidate)
                }
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Narration, \(durationText)")
        .task(id: artifact.id) {
            displayPeaks = WavePlayerBars.displayPeaks(from: SpeechArtifact.peaks(of: artifact))
        }
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

    private var timeText: String {
        "\(Self.clock(isActive ? session.elapsed : 0)) / \(durationText)"
    }

    private var durationText: String {
        let ms = Double(artifact.durationMs)
        if artifact.durationMs < 1000 {
            return String(format: "%.1fs", ms / 1000)
        }
        return Self.clock(ms / 1000)
    }

    private static func clock(_ seconds: TimeInterval) -> String {
        let whole = max(0, Int(seconds.rounded()))
        return String(format: "%d:%02d", whole / 60, whole % 60)
    }
}

struct WavePlayerBars: View {
    let peaks: [Double]
    let fraction: Double
    var height: CGFloat = 26
    var onSeek: ((Double) -> Void)? = nil
    @State private var scrubFraction: Double?

    static func displayPeaks(from source: [Double], barCount: Int = 80) -> [Double] {
        let base = source.isEmpty ? Array(repeating: 0.4, count: barCount) : source
        let resampled = resample(base, to: barCount)
        let low = resampled.min() ?? 0
        let range = max((resampled.max() ?? 1) - low, 0.001)
        return resampled.map { ($0 - low) / range }
    }

    var body: some View {
        GeometryReader { geometry in
            let shown = scrubFraction ?? fraction
            ZStack(alignment: .leading) {
                bars(AnyShapeStyle(Design.ink.opacity(0.2)))
                bars(AnyShapeStyle(Design.accent))
                    .mask(alignment: .leading) {
                        Rectangle()
                            .frame(width: max(0, geometry.size.width * shown))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .animation(
                                scrubFraction == nil ? .linear(duration: 0.11) : nil,
                                value: shown)
                    }
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        guard geometry.size.width > 0, onSeek != nil else { return }
                        scrubFraction = min(max(value.location.x / geometry.size.width, 0), 1)
                    }
                    .onEnded { value in
                        guard geometry.size.width > 0 else { return }
                        let target = min(max(value.location.x / geometry.size.width, 0), 1)
                        scrubFraction = nil
                        onSeek?(target)
                    })
        }
        .frame(height: height)
        .accessibilityHidden(true)
    }

    private func bars(_ style: AnyShapeStyle) -> some View {
        let levels = peaks.isEmpty ? Array(repeating: 0.35, count: 48) : peaks
        return HStack(alignment: .center, spacing: 1.5) {
            ForEach(Array(levels.enumerated()), id: \.offset) { _, level in
                RoundedRectangle.soft(Design.Radius.control)
                    .fill(style)
                    .frame(maxWidth: .infinity)
                    .frame(height: (0.14 + 0.86 * level) * height)
            }
        }
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
                        .clipShape(RoundedRectangle.soft(Design.Radius.card))
                } else if isLoading {
                    ZStack {
                        RoundedRectangle.soft(Design.Radius.card)
                            .fill(Design.cardFill)
                        SkeletonPulse(radius: Design.Radius.card)
                        SheenBand()
                        Image(systemName: "photo")
                            .font(.system(size: 20, weight: .regular))
                            .foregroundStyle(Design.inkFaint)
                    }
                    .aspectRatio(1, contentMode: .fit)
                    .clipShape(RoundedRectangle.soft(Design.Radius.card))
                } else {
                    RoundedRectangle.soft(Design.Radius.card)
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
        .background(Design.cardFill, in: RoundedRectangle.soft(Design.Radius.bubble))
        .hairlineBorder(RoundedRectangle.soft(Design.Radius.bubble))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(
            isLoading
                ? "Image generating"
                : "Generated image" + (caption.map { ", \($0)" } ?? ""))
    }
}

struct ImageDrawingCanvas: View {
    let preview: NSImage?
    let progress: JobProgress
    let statusLine: String?
    let onCancel: () -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var fraction: Double? {
        progress.fraction > 0 ? min(max(progress.fraction, 0), 1) : nil
    }

    private var footerLabel: String? {
        if let step = progress.step, let total = progress.totalSteps {
            return "step \(step) / \(total)"
        }
        return statusLine
    }

    var body: some View {
        ZStack {
            RoundedRectangle.soft(Design.Radius.artifact)
                .fill(Design.cardFill)
            if let preview {
                Image(nsImage: preview)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFill()
                    .opacity(0.5)
            }
            SkeletonPulse(radius: Design.Radius.artifact)
            SheenBand()
            centerCue
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                footer
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .frame(maxWidth: Design.Bubble.imageMax, maxHeight: Design.Bubble.imageMax)
        .clipShape(RoundedRectangle.soft(Design.Radius.artifact))
        .overlay(
            RoundedRectangle.soft(Design.Radius.artifact)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .overlay(alignment: .topTrailing) {
            cancelButton
                .padding(Design.Space.m)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Drawing image" + (statusLine.map { ", \($0)" } ?? ""))
    }

    private var centerCue: some View {
        VStack(spacing: Design.Space.s) {
            Image(systemName: "paintbrush.pointed")
                .font(.system(size: 22, weight: .regular))
                .foregroundStyle(Design.inkFaint)
            ShimmerText(text: "drawing…", font: Design.micro)
            TypingDots()
        }
        .accessibilityHidden(true)
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            if let footerLabel {
                Text(footerLabel)
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .monospacedDigit()
                    .contentTransition(.numericText())
                    .animation(Design.motion(reduceMotion: reduceMotion), value: footerLabel)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            progressBar
        }
        .padding(Design.Space.m)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var progressBar: some View {
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

    private var cancelButton: some View {
        Button(action: onCancel) {
            Image(systemName: "xmark")
                .font(Design.glyphSmall.weight(.bold))
                .foregroundStyle(Design.inkSoft)
                .frame(width: 22, height: 22)
                .background(Design.surface, in: Circle())
                .overlay(Circle().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                .contentShape(Circle())
        }
        .buttonStyle(PressDipStyle())
        .help("Cancel")
        .accessibilityLabel("Cancel image generation")
    }
}

struct ToolTimelineRow: View {
    let summary: String
    let connectsUp: Bool
    let connectsDown: Bool
    let gap: CGFloat

    private let node: CGFloat = 14

    var body: some View {
        HStack(alignment: .center, spacing: Design.Space.chipX) {
            rail
            Text(summary)
                .font(Design.micro)
                .foregroundStyle(Design.inkSoft)
                .lineLimit(1)
                .truncationMode(.middle)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var rail: some View {
        ZStack {
            if connectsUp {
                VStack(spacing: 0) {
                    segment(overshoot: .top)
                    Spacer(minLength: 0)
                }
            }
            if connectsDown {
                VStack(spacing: 0) {
                    Spacer(minLength: 0)
                    segment(overshoot: .bottom)
                }
            }
            Circle()
                .fill(Design.surface)
                .overlay(
                    Circle().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
                .overlay(
                    Image(systemName: "wrench.and.screwdriver")
                        .font(.system(size: 6, weight: .semibold))
                        .foregroundStyle(Design.inkSoft))
                .frame(width: node, height: node)
        }
        .frame(width: node)
        .frame(maxHeight: .infinity)
    }

    private func segment(overshoot edge: Edge.Set) -> some View {
        Rectangle()
            .fill(Design.line)
            .frame(width: Design.hairlineWidth)
            .frame(maxHeight: .infinity)
            .padding(edge, -gap)
    }
}
