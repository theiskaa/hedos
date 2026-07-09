import HedosKernel
import SwiftUI

struct NowPlayingCard: View {
    let session: AudioSession

    var body: some View {
        if let track = session.track {
            card(track)
                .transition(.move(edge: .top).combined(with: .opacity))
        }
    }

    private func card(_ track: AudioSession.Track) -> some View {
        row(track)
            .padding(.leading, Design.Space.xs)
            .padding(.trailing, Design.Space.s)
            .padding(.vertical, Design.Space.xs)
            .surfaceCard(radius: Design.Radius.control)
            .shade(Design.Elevation.lift)
            .help(helpText(track))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Now playing, \(track.subtitle ?? "audio")")
    }

    @ViewBuilder
    private func row(_ track: AudioSession.Track) -> some View {
        if session.phase == .live {
            HStack(spacing: Design.Space.s) {
                transport
                ShimmerText(text: liveLabel(track), font: Design.micro)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: Design.Column.nowPlayingLabel, alignment: .leading)
                dismissButton
            }
            .fixedSize(horizontal: true, vertical: false)
        } else {
            HStack(spacing: Design.Space.s) {
                transport
                ClipScrubber(session: session, peaks: track.peaks)
                ClipTime(session: session)
                rateButton
                dismissButton
            }
            .frame(width: Design.Column.nowPlaying)
        }
    }

    private func liveLabel(_ track: AudioSession.Track) -> String {
        track.subtitle.map { "Speaking · \($0)" } ?? "Generating speech…"
    }

    private func helpText(_ track: AudioSession.Track) -> String {
        let voice = track.subtitle.map { "\($0) · " } ?? ""
        return track.title.isEmpty ? "\(voice)narration" : "\(voice)\(track.title)"
    }

    private var transport: some View {
        Button {
            session.togglePlayback()
        } label: {
            Image(systemName: glyph)
                .font(Design.glyphMicro.weight(.semibold))
                .foregroundStyle(Design.paper)
                .frame(width: 24, height: 24)
                .background(Design.ink, in: RoundedRectangle(cornerRadius: Design.Radius.control))
                .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .help(session.phase == .live ? "Stop" : session.isPaused ? "Play" : "Pause")
        .accessibilityLabel(session.phase == .live ? "Stop" : session.isPaused ? "Play" : "Pause")
    }

    private var glyph: String {
        if session.phase == .live { return "stop.fill" }
        return session.isPaused ? "play.fill" : "pause.fill"
    }

    private var rateButton: some View {
        Button {
            session.cycleRate()
        } label: {
            Text(session.rateLabel)
                .font(Design.micro)
                .monospacedDigit()
                .foregroundStyle(Design.inkSoft)
                .lineLimit(1)
                .fixedSize()
                .frame(minWidth: 26, alignment: .trailing)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Playback speed")
        .accessibilityLabel("Playback speed \(session.rateLabel)")
    }

    private var dismissButton: some View {
        Button {
            session.dismiss()
        } label: {
            Image(systemName: "xmark")
                .font(Design.glyphMicro)
                .foregroundStyle(Design.inkFaint)
                .frame(width: 16, height: 16)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Dismiss")
        .accessibilityLabel("Dismiss now playing")
    }

}

private struct ClipScrubber: View {
    let session: AudioSession
    let peaks: [Double]

    var body: some View {
        WavePlayerBars(
            peaks: peaks,
            fraction: session.progress,
            height: 14,
            barCount: 28,
            onSeek: { session.seek(to: $0) })
    }
}

private struct ClipTime: View {
    let session: AudioSession

    var body: some View {
        Text("\(clock(session.elapsed)) / \(clock(session.duration))")
            .font(Design.data(9))
            .monospacedDigit()
            .foregroundStyle(Design.inkSoft)
            .fixedSize()
    }

    private func clock(_ seconds: TimeInterval) -> String {
        let whole = max(0, Int(seconds.rounded()))
        return String(format: "%d:%02d", whole / 60, whole % 60)
    }
}
