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
        HStack(spacing: Design.Space.l) {
            CircleControl(
                glyph: isSounding ? "pause.fill" : "play.fill",
                prominent: true,
                label: isSounding ? "Pause" : isActive ? "Resume" : "Play",
                action: onToggle)
            WaveformBars(
                peaks: VoiceSurfaceModel.peaks(of: artifact),
                emphasized: isActive,
                progress: isActive ? clips.progress : nil,
                onSeek: isActive ? { clips.seek(to: $0) } : nil)
            VStack(alignment: .trailing, spacing: Design.Space.xxs) {
                Text(timeText)
                    .font(Design.data(10))
                    .foregroundStyle(Design.inkSoft)
                    .monospacedDigit()
                if let voice = VoiceSurfaceModel.voiceName(of: artifact) {
                    TintChip(text: voice, live: isSounding)
                }
            }
        }
        .responseShell()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Spoken recording, \(durationText)")
    }

    private var timeText: String {
        isActive ? "\(Self.clock(clips.elapsed)) · \(durationText)" : durationText
    }

    private var durationText: String {
        Self.clock(Double(max(1000, artifact.durationMs)) / 1000)
    }

    private static func clock(_ seconds: TimeInterval) -> String {
        let whole = max(0, Int(seconds.rounded()))
        return String(format: "%d:%02d", whole / 60, whole % 60)
    }
}

struct WaveformBars: View {
    let peaks: [Double]
    let emphasized: Bool
    var progress: Double? = nil
    var onSeek: ((Double) -> Void)? = nil

    var body: some View {
        HStack(alignment: .center, spacing: 2) {
            ForEach(Array(displayPeaks.enumerated()), id: \.offset) { index, peak in
                Capsule()
                    .fill(barStyle(index))
                    .frame(width: 2.5, height: 4 + CGFloat(peak) * 18)
            }
        }
        .frame(height: 24)
        .overlay {
            if let onSeek {
                GeometryReader { geometry in
                    Color.clear
                        .contentShape(Rectangle())
                        .gesture(
                            DragGesture(minimumDistance: 0)
                                .onChanged { value in
                                    guard geometry.size.width > 0 else { return }
                                    onSeek(value.location.x / geometry.size.width)
                                })
                }
            }
        }
        .accessibilityHidden(true)
    }

    private func barStyle(_ index: Int) -> AnyShapeStyle {
        guard emphasized else { return AnyShapeStyle(Design.inkSoft) }
        guard let progress else { return AnyShapeStyle(Design.ink) }
        let played = Double(index + 1) / Double(max(1, displayPeaks.count)) <= progress
        return AnyShapeStyle(played ? Design.accent : Design.inkFaint)
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
