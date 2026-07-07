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
            .background(Design.bubbleFill, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
            .frame(maxWidth: 520, alignment: .trailing)
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
    let isPlaying: Bool
    let onTogglePlayback: () -> Void

    var body: some View {
        HStack(spacing: Design.Space.l) {
            CircleControl(
                glyph: isPlaying ? "pause.fill" : "play.fill",
                prominent: true,
                label: isPlaying ? "Pause" : "Play",
                action: onTogglePlayback)
            WaveformBars(
                peaks: VoiceSurfaceModel.peaks(of: artifact),
                emphasized: isPlaying)
            VStack(alignment: .trailing, spacing: Design.Space.xxs) {
                Text(durationText)
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkSoft)
                    .monospacedDigit()
                if let voice = VoiceSurfaceModel.voiceName(of: artifact) {
                    Text(voice.uppercased())
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                }
            }
        }
        .responseShell()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Spoken recording, \(durationText)")
    }

    private var durationText: String {
        let seconds = max(1, artifact.durationMs / 1000)
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}

struct WaveformBars: View {
    let peaks: [Double]
    let emphasized: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 2) {
            ForEach(Array(displayPeaks.enumerated()), id: \.offset) { _, peak in
                Capsule()
                    .fill(emphasized ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.inkSoft))
                    .frame(width: 2.5, height: 4 + CGFloat(peak) * 18)
            }
        }
        .frame(height: 24)
        .accessibilityHidden(true)
    }

    private var displayPeaks: [Double] {
        peaks.isEmpty ? Array(repeating: 0.4, count: 28) : peaks
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
                } else {
                    RoundedRectangle(cornerRadius: Design.Radius.card)
                        .fill(Design.cardFill)
                        .overlay {
                            if isLoading {
                                ProgressView()
                                    .controlSize(.small)
                            }
                        }
                        .aspectRatio(1, contentMode: .fit)
                }
            }
            .frame(maxWidth: 360, maxHeight: 360)
            if let caption, !caption.isEmpty {
                Text(caption)
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 360)
            }
        }
        .padding(Design.Space.m)
        .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.bubble))
        .overlay(
            RoundedRectangle(cornerRadius: Design.Radius.bubble)
                .strokeBorder(Design.hairline, lineWidth: Design.hairlineWidth))
    }
}
