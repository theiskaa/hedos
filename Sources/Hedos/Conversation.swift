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
    @Environment(\.conversationWidth) private var conversationWidth
    @Environment(\.sendWithEnter) private var sendWithEnter
    @State private var composerHeight: CGFloat = 22

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
            ZStack(alignment: .topLeading) {
                if draft.isEmpty {
                    Text(placeholder)
                        .font(Design.body)
                        .foregroundStyle(Design.inkFaint)
                        .padding(.leading, 3)
                        .padding(.top, 2)
                        .allowsHitTesting(false)
                }
                ComposerTextView(
                    text: $draft,
                    sendWithEnter: sendWithEnter,
                    measuredHeight: $composerHeight
                ) {
                    if canSend && !isWorking {
                        onSend()
                    }
                }
                .frame(height: composerHeight)
                .accessibilityLabel(placeholder)
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
                }
            }
        }
        .padding(Design.Space.l)
        .surfaceCard()
        .shadow(color: Design.shadowColor.opacity(0.12), radius: 24, x: 0, y: 10)
        .frame(maxWidth: conversationWidth)
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

private struct ComposerTextView: NSViewRepresentable {
    @Binding var text: String
    let sendWithEnter: Bool
    @Binding var measuredHeight: CGFloat
    let onSend: () -> Void

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerTextView

        init(parent: ComposerTextView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let view = notification.object as? NSTextView else { return }
            parent.text = view.string
            parent.remeasure(view)
        }

        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            guard selector == #selector(NSResponder.insertNewline(_:)) else { return false }
            let flags = NSApp.currentEvent?.modifierFlags ?? []
            if flags.contains(.command) {
                parent.onSend()
                return true
            }
            if flags.contains(.shift) || !parent.sendWithEnter {
                return false
            }
            parent.onSend()
            return true
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        if let view = scroll.documentView as? NSTextView {
            view.delegate = context.coordinator
            view.font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
            view.drawsBackground = false
            view.isRichText = false
            view.allowsUndo = true
            view.textContainerInset = NSSize(width: 0, height: 2)
            view.textContainer?.lineFragmentPadding = 3
            view.string = text
        }
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let view = scroll.documentView as? NSTextView else { return }
        if view.string != text {
            view.string = text
        }
        let ink = NSColor(Design.ink)
        if view.textColor != ink {
            view.textColor = ink
            view.insertionPointColor = ink
        }
        remeasure(view)
    }

    func remeasure(_ view: NSTextView) {
        guard let container = view.textContainer, let manager = view.layoutManager else { return }
        manager.ensureLayout(for: container)
        let inset = view.textContainerInset.height * 2
        let used = manager.usedRect(for: container).height + inset
        let line = view.font?.boundingRectForFont.height ?? 18
        let floor = ceil(line) + inset
        let ceilHeight = ceil(line * 6) + inset
        let target = min(max(used, floor), ceilHeight)
        guard abs(measuredHeight - target) > 0.5 else { return }
        DispatchQueue.main.async {
            measuredHeight = target
        }
    }
}
