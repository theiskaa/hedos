import AVFoundation
import AppKit
import HedosKernel
import SwiftUI

struct ConversationScaffold<Transcript: View, Aux: View, Chip: View>: View {
    let placeholder: String
    @Binding var draft: String
    let isWorking: Bool
    let canSend: Bool
    let notice: String?
    var noticeActionLabel: String? = nil
    var noticeAction: (() -> Void)? = nil
    let onSend: () -> Void
    let onStop: () -> Void
    @ViewBuilder let transcript: () -> Transcript
    @ViewBuilder let aux: () -> Aux
    @ViewBuilder let chip: () -> Chip
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 0) {
            transcript()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let notice {
                noticeBar(notice)
            }
            composer
        }
    }

    private func noticeBar(_ text: String) -> some View {
        HStack(spacing: Design.Space.m) {
            Image(systemName: "exclamationmark.triangle")
                .font(Design.glyphInline.weight(.semibold))
                .foregroundStyle(Design.inkSoft)
            Text(text)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(Design.inkSoft)
            if let noticeActionLabel, let noticeAction {
                Button(noticeActionLabel, action: noticeAction)
                    .buttonStyle(QuietButtonStyle())
            }
            Spacer()
        }
        .padding(.horizontal, Design.Space.pane)
        .padding(.bottom, Design.Space.s)
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            TextField(placeholder, text: $draft, axis: .vertical)
                .textFieldStyle(.plain)
                .font(Design.body)
                .lineLimit(1...6)
                .onSubmit {
                    if canSend && !isWorking {
                        onSend()
                    }
                }
                .onKeyPress(.return, phases: .down) { press in
                    guard press.modifiers.contains(.shift) else { return .ignored }
                    draft += "\n"
                    return .handled
                }
                .padding(.top, Design.Space.xs)
                .padding(.horizontal, Design.Space.xs)
            HStack(spacing: Design.Space.m) {
                Spacer(minLength: 0)
                aux()
                chip()
                if isWorking {
                    CircleControl(
                        glyph: "stop.fill", prominent: true, label: "Stop", action: onStop
                    )
                    .keyboardShortcut(.cancelAction)
                } else {
                    CircleControl(
                        glyph: "arrow.up", prominent: true, label: "Send", action: onSend
                    )
                    .disabled(!canSend)
                    .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(Design.Space.l)
        .surfaceCard()
        .shadow(color: Design.shadowColor.opacity(0.12), radius: 24, x: 0, y: 10)
        .frame(maxWidth: Design.conversationMaxWidth)
        .frame(maxWidth: .infinity)
        .padding(.horizontal, Design.Space.xxl)
        .padding(.bottom, Design.Space.xxl)
        .padding(.top, Design.Space.m)
        .animation(Design.motion(reduceMotion: reduceMotion), value: isWorking)
    }
}

struct CircleControl: View {
    let glyph: String
    var prominent = false
    let label: String
    let action: () -> Void
    @Environment(\.isEnabled) private var isEnabled

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: glyph)
                .font(Design.caption.weight(.semibold))
                .foregroundStyle(prominent ? Design.paper : Design.inkSoft)
                .frame(width: 28, height: 28)
                .background(
                    prominent ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface),
                    in: Circle())
                .overlay {
                    if prominent {
                        Circle()
                            .strokeBorder(
                                LinearGradient(
                                    colors: [Color.white.opacity(0.16), .clear],
                                    startPoint: .top, endPoint: .center),
                                lineWidth: 1)
                    } else {
                        Circle()
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth)
                    }
                }
                .shadow(
                    color: prominent
                        ? Design.shadowColor.opacity(hovering ? 0.30 : 0.20) : .clear,
                    radius: hovering ? 14 : 9,
                    x: 0,
                    y: hovering ? 7 : 4)
                .offset(y: hovering && prominent ? -1 : 0)
                .contentShape(Circle())
                .opacity(isEnabled ? 1 : 0.4)
                .animation(.easeOut(duration: 0.2), value: hovering)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}

struct ChipMenu<Content: View>: View {
    let title: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        Menu {
            content()
        } label: {
            HStack(spacing: Design.Space.xs) {
                Text(title)
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(Design.glyphSmall)
            }
            .font(Design.label)
            .foregroundStyle(Design.inkSoft)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.xs)
            .background(Design.surface, in: Capsule())
            .overlay(Capsule().strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
    }
}

@MainActor
private final class ArtifactBubblePlayback: NSObject, AVAudioPlayerDelegate {
    var player: AVAudioPlayer?
    var onFinish: (() -> Void)?

    nonisolated func audioPlayerDidFinishPlaying(
        _ player: AVAudioPlayer, successfully flag: Bool
    ) {
        Task { @MainActor in
            self.onFinish?()
        }
    }
}

struct ArtifactExchangeView: View {
    let reference: String
    let kernel: Kernel
    @State private var artifact: Artifact?
    @State private var image: NSImage?
    @State private var isPlaying = false
    @State private var isLoadingPlayback = false
    @State private var playback = ArtifactBubblePlayback()

    var body: some View {
        Group {
            if let artifact {
                switch artifact.capability {
                case .speak:
                    VoiceBubble(artifact: artifact, isPlaying: isPlaying) {
                        togglePlayback(artifact)
                    }
                case .image:
                    ImageBubble(
                        image: image,
                        caption: Provenance.line(for: artifact),
                        isLoading: image == nil)
                default:
                    EmptyView()
                }
            }
        }
        .task(id: reference) {
            artifact = try? await kernel.artifact(id: reference)
            if artifact?.capability == .image, image == nil {
                if let data = try? await kernel.artifactPreview(id: reference) {
                    image = NSImage(data: data)
                } else if let url = try? await kernel.artifactURL(id: reference) {
                    image = NSImage(contentsOf: url)
                }
            }
        }
    }

    private func togglePlayback(_ artifact: Artifact) {
        if isPlaying {
            playback.player?.stop()
            playback.player = nil
            isPlaying = false
            return
        }
        guard !isLoadingPlayback else { return }
        isLoadingPlayback = true
        let kernel = kernel
        Task { @MainActor in
            defer { isLoadingPlayback = false }
            guard let url = try? await kernel.artifactURL(id: artifact.id),
                let player = try? AVAudioPlayer(contentsOf: url)
            else { return }
            playback.player = player
            playback.onFinish = {
                isPlaying = false
            }
            player.delegate = playback
            player.play()
            isPlaying = true
        }
    }
}
