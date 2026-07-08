import AVFoundation
import AppKit
import HedosKernel
import SwiftUI
import UniformTypeIdentifiers

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
    var slash: SlashSetup? = nil
    var dictation: DictationSetup? = nil
    @ViewBuilder let transcript: () -> Transcript
    @ViewBuilder let aux: () -> Aux
    @ViewBuilder let chip: () -> Chip
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.conversationWidth) private var conversationWidth
    @Environment(\.sendWithEnter) private var sendWithEnter
    @State private var composerHeight: CGFloat = 22
    @State private var composerFocusToken = 0
    @State private var slashPrompts: [Prompt] = []
    @State private var slashHighlight = 0
    @State private var slashSuppressed = false
    @State private var dictationController = DictationController()

    var body: some View {
        VStack(spacing: 0) {
            transcript()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            if let notice {
                noticeBar(notice)
            }
            composer
        }
        .onAppear { composerFocusToken += 1 }
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
                    measuredHeight: $composerHeight,
                    onCommand: interceptCommand,
                    focusToken: composerFocusToken
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
                if let notice = dictationController.notice {
                    Text(notice)
                        .font(Design.label)
                        .foregroundStyle(Design.inkSoft)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Spacer(minLength: 0)
                micControl
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
        .shade(Design.Elevation.lift)
        .overlay(alignment: .top) {
            Color.clear
                .frame(height: 1)
                .overlay(alignment: .bottom) {
                    if slashActive && !slashEntries.isEmpty {
                        SlashMenuPanel(
                            entries: slashEntries,
                            highlighted: slashHighlight,
                            onAccept: acceptSlash,
                            onHighlight: { slashHighlight = $0 }
                        )
                        .offset(y: -Design.Space.s)
                    }
                }
        }
        .frame(maxWidth: conversationWidth, alignment: .leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, Design.Space.xxl)
        .padding(.bottom, Design.Space.xxl)
        .padding(.top, Design.Space.m)
        .animation(Design.motion(reduceMotion: reduceMotion), value: isWorking)
        .onChange(of: slashQuery) { _, query in
            slashHighlight = 0
            if query == nil {
                slashSuppressed = false
            }
        }
        .task(id: slashActive) {
            guard slashActive, let slash else { return }
            slashPrompts = await slash.kernel.prompts()
        }
    }

    @ViewBuilder
    private var micControl: some View {
        if let dictation, DictationController.transcriber(in: dictation.records()) != nil {
            CircleControl(
                glyph: dictationController.phase == .recording
                    ? "stop.fill"
                    : dictationController.phase == .transcribing ? "ellipsis" : "mic",
                prominent: dictationController.phase == .recording,
                live: dictationController.phase == .recording,
                label: dictationController.phase == .recording
                    ? "Stop dictation"
                    : dictationController.phase == .transcribing
                        ? "Cancel transcription" : "Dictate"
            ) {
                dictationController.toggle(setup: dictation) { delta in
                    draft += delta
                }
            }
            .accessibilityIdentifier("composer-mic")
        }
    }

    private var slashQuery: String? {
        guard slash != nil, !isWorking else { return nil }
        return PromptComposer.query(in: draft)
    }

    private var slashActive: Bool {
        slashQuery != nil && !slashSuppressed
    }

    private var slashEntries: [SlashEntry] {
        guard let slash, let query = slashQuery, !slashSuppressed else { return [] }
        return SlashMenu.entries(
            query: query, prompts: slashPrompts, commands: slash.commands,
            capability: slash.capability)
    }

    private func acceptSlash(_ entry: SlashEntry) {
        switch entry.kind {
        case .command(let command):
            draft = PromptComposer.clearingToken(from: draft)
            command.perform()
        case .prompt(let prompt):
            draft = PromptComposer.inserting(prompt, into: draft)
        }
    }

    private func interceptCommand(_ selector: Selector) -> Bool {
        guard slashActive else { return false }
        let entries = slashEntries
        guard !entries.isEmpty else { return false }
        switch selector {
        case #selector(NSResponder.moveUp(_:)):
            slashHighlight = (slashHighlight - 1 + entries.count) % entries.count
            return true
        case #selector(NSResponder.moveDown(_:)):
            slashHighlight = (slashHighlight + 1) % entries.count
            return true
        case #selector(NSResponder.insertNewline(_:)), #selector(NSResponder.insertTab(_:)):
            acceptSlash(entries[min(slashHighlight, entries.count - 1)])
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            slashSuppressed = true
            return true
        default:
            return false
        }
    }
}

struct TranscriptEmptyState: View {
    let eyebrow: String
    let headline: String
    let caption: String

    var body: some View {
        VStack(spacing: Design.Space.m) {
            Text(eyebrow.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
            Text(headline)
                .font(Design.title)
                .tracking(Design.tightTracking)
                .foregroundStyle(Design.ink)
            Text(caption)
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .multilineTextAlignment(.center)
                .lineSpacing(2.5)
                .frame(maxWidth: Design.Column.emptyCaption)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 120)
        .accessibilityElement(children: .combine)
    }
}

struct CircleControl: View {
    let glyph: String
    var prominent = false
    var live = false
    let label: String
    let action: () -> Void
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.colorScheme) private var colorScheme

    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: glyph)
                .font(Design.caption.weight(.semibold))
                .foregroundStyle(prominent ? Design.paper : Design.inkSoft)
                .frame(width: 28, height: 28)
                .background(
                    live
                        ? AnyShapeStyle(Design.accent)
                        : prominent ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface),
                    in: RoundedRectangle(cornerRadius: Design.Radius.control))
                .overlay {
                    if prominent || live {
                        RoundedRectangle(cornerRadius: Design.Radius.control)
                            .strokeBorder(
                                LinearGradient(
                                    colors: [
                                        Color.white.opacity(colorScheme == .dark ? 0.10 : 0.18),
                                        .clear,
                                    ],
                                    startPoint: .top, endPoint: .center),
                                lineWidth: Design.hairlineWidth)
                    } else {
                        RoundedRectangle(cornerRadius: Design.Radius.control)
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth)
                    }
                }
                .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
                .opacity(isEnabled ? 1 : 0.4)
        }
        .buttonStyle(CirclePressStyle(prominent: prominent, hovering: hovering))
        .onHover { hovering = $0 }
        .inkFocusRing(RoundedRectangle(cornerRadius: Design.Radius.control))
        .help(label)
        .accessibilityLabel(label)
    }
}

private struct CirclePressStyle: ButtonStyle {
    let prominent: Bool
    let hovering: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .shadow(
                color: prominent
                    ? Design.shadowColor.opacity(
                        configuration.isPressed
                            ? 0.14
                            : hovering
                                ? Design.Elevation.buttonHover.opacity
                                : Design.Elevation.button.opacity)
                    : .clear,
                radius: hovering && !configuration.isPressed
                    ? Design.Elevation.buttonHover.radius : Design.Elevation.button.radius,
                x: 0,
                y: configuration.isPressed
                    ? 4
                    : hovering ? Design.Elevation.buttonHover.y : Design.Elevation.button.y)
            .scaleEffect(configuration.isPressed ? 0.94 : 1)
            .offset(y: configuration.isPressed ? 0 : hovering && prominent ? -1 : 0)
            .animation(.easeOut(duration: 0.2), value: hovering)
            .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
    }
}

struct ArtifactExchangeView: View {
    let reference: String
    let kernel: Kernel
    @State private var artifact: Artifact?
    @State private var image: NSImage?
    @State private var clips = AudioClipController()

    var body: some View {
        Group {
            if let artifact {
                switch artifact.capability {
                case .speak:
                    VStack(alignment: .leading, spacing: Design.Space.s) {
                        artifactLabel(
                            kind: "voice", name: VoiceSurfaceModel.voiceName(of: artifact))
                        VoiceBubble(artifact: artifact, clips: clips) {
                            togglePlayback(artifact)
                        }
                        ArtifactTray {
                            TrayButton(label: "Save audio", glyph: "arrow.down.to.line") {
                                exportArtifact(artifact, suggested: "narration.wav")
                            }
                            TrayButton(label: "Transcript", glyph: "doc.on.doc") {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(
                                    VoiceSurfaceModel.text(of: artifact), forType: .string)
                            }
                        }
                    }
                case .image:
                    VStack(alignment: .leading, spacing: Design.Space.s) {
                        ImageBubble(
                            image: image,
                            caption: Provenance.line(for: artifact),
                            isLoading: image == nil)
                        if image != nil {
                            ArtifactTray {
                                TrayButton(label: "Save .png", glyph: "arrow.down.to.line") {
                                    saveImage()
                                }
                                TrayButton(label: "Copy", glyph: "doc.on.doc") {
                                    copyImage()
                                }
                            }
                        }
                    }
                default:
                    EmptyView()
                }
            }
        }
        .task(id: reference) {
            artifact = try? await kernel.artifact(id: reference)
            if artifact?.capability == .image, image == nil {
                let scale = NSScreen.main?.backingScaleFactor ?? 2
                if let url = try? await kernel.artifactURL(id: reference),
                    let sharp = ImagesViewModel.downsampled(
                        url, maxPixel: Design.Bubble.imageMax * scale)
                {
                    image = sharp
                } else if let data = try? await kernel.artifactPreview(id: reference) {
                    image = NSImage(data: data)
                }
            }
        }
        .onDisappear {
            clips.stop()
        }
    }

    private func artifactLabel(kind: String, name: String?) -> some View {
        HStack(spacing: Design.Space.xs) {
            Text(verbatim: "▸")
                .font(Design.micro)
                .foregroundStyle(Design.accent)
            Text((name.map { "\(kind) · \($0)" } ?? kind).uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
        }
    }

    private func togglePlayback(_ artifact: Artifact) {
        if clips.isActive(artifact.id) {
            clips.toggle(id: artifact.id)
            return
        }
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactURL(id: artifact.id) else { return }
            clips.toggle(id: artifact.id, url: url)
        }
    }

    private func exportArtifact(_ artifact: Artifact, suggested: String) {
        let kernel = kernel
        Task { @MainActor in
            guard let source = try? await kernel.artifactURL(id: artifact.id),
                let data = try? Data(contentsOf: source)
            else { return }
            let panel = NSSavePanel()
            panel.nameFieldStringValue = source.lastPathComponent.isEmpty
                ? suggested : source.lastPathComponent
            guard panel.runModal() == .OK, let destination = panel.url else { return }
            try? data.write(to: destination)
        }
    }

    private func saveImage() {
        guard let image, let data = ArtifactExchangeView.pngData(image) else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.nameFieldStringValue = "image.png"
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        try? data.write(to: destination)
    }

    private func copyImage() {
        guard let image else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([image])
    }

    private static func pngData(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation,
            let rep = NSBitmapImageRep(data: tiff)
        else { return nil }
        return rep.representation(using: .png, properties: [:])
    }
}

struct ArtifactTray<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        HStack(spacing: Design.Space.s) {
            content()
        }
    }
}

struct TrayButton: View {
    let label: String
    let glyph: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Design.Space.xs) {
                Image(systemName: glyph)
                    .font(Design.glyphSmall)
                Text(label)
                    .font(Design.micro)
                    .tracking(0.4)
            }
            .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.xs + 1)
            .background(Design.panel, in: RoundedRectangle(cornerRadius: Design.Radius.control))
            .overlay(
                RoundedRectangle(cornerRadius: Design.Radius.control)
                    .strokeBorder(
                        hovering ? Design.lineBright : Design.line,
                        lineWidth: Design.hairlineWidth))
            .contentShape(RoundedRectangle(cornerRadius: Design.Radius.control))
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .accessibilityLabel(label)
    }
}

private struct ComposerTextView: NSViewRepresentable {
    @Binding var text: String
    let sendWithEnter: Bool
    @Binding var measuredHeight: CGFloat
    var onCommand: ((Selector) -> Bool)? = nil
    var focusToken: Int = 0
    let onSend: () -> Void

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: ComposerTextView
        var lastFocusToken = -1

        init(parent: ComposerTextView) {
            self.parent = parent
        }

        func textDidChange(_ notification: Notification) {
            guard let view = notification.object as? NSTextView else { return }
            parent.text = view.string
            parent.remeasure(view)
        }

        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            if let onCommand = parent.onCommand, onCommand(selector) {
                return true
            }
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
            view.font = Design.editorFont()
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
        if context.coordinator.lastFocusToken != focusToken {
            context.coordinator.lastFocusToken = focusToken
            DispatchQueue.main.async {
                scroll.window?.makeFirstResponder(view)
            }
        }
        if view.string != text {
            view.string = text
            view.setSelectedRange(NSRange(location: (text as NSString).length, length: 0))
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
