import AVFoundation
import AppKit
import HedosKernel
import SwiftUI
import UniformTypeIdentifiers

struct MentionSetup {
    let files: () async -> [String]
}

private let composerCornerRadius: CGFloat = 22

struct ConversationScaffold<Transcript: View, Header: View, Aux: View, Chip: View>: View {
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
    var mentions: MentionSetup? = nil
    var dictation: DictationSetup? = nil
    @ViewBuilder let transcript: () -> Transcript
    @ViewBuilder let header: () -> Header
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
    @State private var mentionFiles: [String] = []
    @State private var mentionIndex: Set<String> = []
    @State private var mentionSuppressed = false
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
            header()
                .padding(.horizontal, Design.Space.s)
            VStack(alignment: .leading, spacing: 0) {
                if accessoryActive {
                    composerAccessory
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(Design.Space.s)
                        .transition(.opacity)
                    Rectangle()
                        .fill(Design.line)
                        .frame(height: Design.hairlineWidth)
                        .transition(.opacity)
                }
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
                            focusToken: composerFocusToken,
                            mentionPaths: mentionIndex
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
            }
            .surfaceCard(radius: composerCornerRadius)
            .clipShape(RoundedRectangle.soft(composerCornerRadius))
            .shade(Design.Elevation.raised)
            .animation(Design.spring, value: menuActive)
            .animation(Design.spring, value: ConsentCoordinator.shared.pending?.id)
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
        .onChange(of: mentionQuery) { _, query in
            slashHighlight = 0
            if query == nil {
                mentionSuppressed = false
            }
        }
        .task(id: slashActive) {
            guard slashActive, let slash else { return }
            slashPrompts = await slash.kernel.promptStore.list()
        }
        .task(id: mentionActive) {
            guard mentionActive, let mentions else { return }
            mentionFiles = await mentions.files()
            mentionIndex = Set(mentionFiles)
        }
        .task {
            guard let mentions else { return }
            mentionIndex = Set(await mentions.files())
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
        guard !mentionWins else { return nil }
        return PromptComposer.query(in: draft)
    }

    private var mentionQuery: String? {
        guard mentions != nil, !isWorking, mentionWins else { return nil }
        return PromptComposer.mentionQuery(in: draft)
    }

    private var mentionWins: Bool {
        guard mentions != nil,
            let mention = PromptComposer.mentionRange(in: draft)
        else { return false }
        guard let token = PromptComposer.tokenRange(in: draft) else { return true }
        return mention.lowerBound > token.lowerBound
    }

    private var slashActive: Bool {
        slashQuery != nil && !slashSuppressed
    }

    private var mentionActive: Bool {
        mentionQuery != nil && !mentionSuppressed
    }

    private var menuActive: Bool {
        slashActive || mentionActive
    }

    private var accessoryActive: Bool {
        ConsentCoordinator.shared.pending != nil || (menuActive && !menuEntries.isEmpty)
    }

    private var slashEntries: [SlashEntry] {
        guard let slash, let query = slashQuery, !slashSuppressed else { return [] }
        return SlashMenu.entries(
            query: query, prompts: slashPrompts, commands: slash.commands,
            capability: slash.capability)
    }

    private var mentionEntries: [SlashEntry] {
        guard let query = mentionQuery, !mentionSuppressed else { return [] }
        return PlaceFiles.matches(query: query, in: mentionFiles)
            .map { SlashEntry(kind: .file($0)) }
    }

    private var menuEntries: [SlashEntry] {
        mentionActive ? mentionEntries : slashEntries
    }

    @ViewBuilder
    private var composerAccessory: some View {
        if let pending = ConsentCoordinator.shared.pending {
            ConsentCard(pending: pending) { decision in
                ConsentCoordinator.shared.decide(pending, decision)
            }
        } else if menuActive && !menuEntries.isEmpty {
            SlashMenuPanel(
                entries: menuEntries,
                highlighted: slashHighlight,
                onAccept: acceptSlash,
                onHighlight: { slashHighlight = $0 }
            )
        }
    }

    private func acceptSlash(_ entry: SlashEntry) {
        switch entry.kind {
        case .command(let command):
            draft = PromptComposer.clearingToken(from: draft)
            command.perform()
        case .prompt(let prompt):
            draft = PromptComposer.inserting(prompt, into: draft)
        case .file(let path):
            draft = PromptComposer.acceptingMention(path, in: draft)
        }
    }

    private func interceptCommand(_ selector: Selector) -> Bool {
        guard menuActive else { return false }
        let entries = menuEntries
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
            if mentionActive {
                mentionSuppressed = true
            } else {
                slashSuppressed = true
            }
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
        VStack(spacing: Design.Space.l) {
            HedosLogo(size: 52, color: Design.inkSoft)
                .opacity(0.9)
                .padding(.bottom, Design.Space.xs)
            VStack(spacing: Design.Space.m) {
                Text(eyebrow)
                    .font(Design.micro)
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
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 96)
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
                .frame(width: Design.Control.size, height: Design.Control.size)
                .background(
                    live
                        ? AnyShapeStyle(Design.accent)
                        : prominent ? AnyShapeStyle(Design.ink) : AnyShapeStyle(Design.surface),
                    in: Circle())
                .overlay {
                    if prominent || live {
                        Circle()
                            .strokeBorder(
                                LinearGradient(
                                    colors: [
                                        Color.white.opacity(colorScheme == .dark ? 0.10 : 0.18),
                                        .clear,
                                    ],
                                    startPoint: .top, endPoint: .center),
                                lineWidth: Design.hairlineWidth)
                    } else {
                        Circle()
                            .strokeBorder(Design.line, lineWidth: Design.hairlineWidth)
                    }
                }
                .contentShape(Circle())
                .opacity(isEnabled ? 1 : 0.4)
        }
        .buttonStyle(CirclePressStyle(prominent: prominent, hovering: hovering))
        .onHover { hovering = $0 }
        .inkFocusRing(Circle())
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
    let session: AudioSession
    var onRerun: ((Artifact) -> Void)? = nil
    var onVary: ((Artifact) -> Void)? = nil
    @State private var artifact: Artifact?
    @State private var image: NSImage?
    @State private var resolved = false
    @State private var saveNotice: String?
    @State private var copiedImage = false

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            content
            if let saveNotice {
                Text(saveNotice)
                    .font(Design.label)
                    .foregroundStyle(Design.heatText)
                    .lineLimit(2)
            }
        }
        .task(id: reference) {
            resolved = false
            artifact = try? await kernel.artifactStore.get(id: reference)
            resolved = true
            if artifact?.capability == .image, image == nil {
                let scale = NSScreen.main?.backingScaleFactor ?? 2
                let maxPixel = Design.Bubble.imageMax * scale
                var sharp: NSImage?
                if let url = try? await kernel.artifactStore.url(id: reference) {
                    sharp = await Task.detached(priority: .utility) {
                        GalleryModel.downsampled(url, maxPixel: maxPixel)
                    }.value
                }
                if let sharp {
                    image = sharp
                } else if let data = try? await kernel.artifactStore.previewData(id: reference) {
                    image = await Task.detached(priority: .utility) { NSImage(data: data) }.value
                }
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        Group {
            if let artifact {
                switch artifact.capability {
                case .speak:
                    VStack(alignment: .leading, spacing: Design.Space.s) {
                        artifactLabel(
                            kind: "voice", name: SpeechArtifact.voiceName(of: artifact))
                        VoiceBubble(artifact: artifact, session: session) {
                            session.toggle(artifact)
                        }
                        ArtifactTray {
                            TrayButton(label: "Save audio", glyph: "arrow.down.to.line") {
                                exportArtifact(artifact, suggested: "narration.wav")
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
                                if let onRerun {
                                    TrayButton(label: "Re-run", glyph: "arrow.clockwise") {
                                        onRerun(artifact)
                                    }
                                }
                                if let onVary {
                                    TrayButton(label: "Vary", glyph: "wand.and.sparkles") {
                                        onVary(artifact)
                                    }
                                }
                                TrayButton(label: "Save .png", glyph: "arrow.down.to.line") {
                                    saveImage()
                                }
                                TrayButton(
                                    label: copiedImage ? "Copied" : "Copy",
                                    glyph: copiedImage ? "checkmark" : "doc.on.doc"
                                ) {
                                    copyImage()
                                    copiedImage = true
                                    Task {
                                        try? await Task.sleep(for: .seconds(1.5))
                                        copiedImage = false
                                    }
                                }
                            }
                        }
                    }
                default:
                    Color.clear.frame(height: 1)
                }
            } else if resolved {
                missingArtifact
            } else {
                SkeletonPulse()
                    .frame(width: Design.Bubble.artifactPlaceholder.width,
                        height: Design.Bubble.artifactPlaceholder.height)
                    .clipShape(RoundedRectangle.soft(Design.Radius.artifact))
            }
        }
    }

    private var missingArtifact: some View {
        Label {
            Text("This attachment was deleted.")
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
        } icon: {
            Image(systemName: "trash.slash")
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkFaint)
        }
        .frame(width: Design.Bubble.artifactPlaceholder.width, alignment: .leading)
        .padding(.vertical, Design.Space.s)
        .accessibilityLabel("Attachment deleted")
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

    private func exportArtifact(_ artifact: Artifact, suggested: String) {
        let kernel = kernel
        Task { @MainActor in
            guard let source = try? await kernel.artifactStore.url(id: artifact.id) else {
                saveNotice = "This artifact's file is missing."
                return
            }
            let panel = NSSavePanel()
            panel.nameFieldStringValue = source.lastPathComponent.isEmpty
                ? suggested : source.lastPathComponent
            guard panel.runModal() == .OK, let destination = panel.url else { return }
            do {
                try await Task.detached(priority: .utility) {
                    try AtomicFileWrite.copy(from: source, to: destination)
                }.value
                saveNotice = nil
            } catch {
                saveNotice = error.localizedDescription
            }
        }
    }

    private func saveImage() {
        guard let image, let data = ArtifactExchangeView.pngData(image) else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.nameFieldStringValue = "image.png"
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        do {
            try data.write(to: destination, options: .atomic)
            saveNotice = nil
        } catch {
            saveNotice = error.localizedDescription
        }
    }

    private func copyImage() {
        guard let image else { return }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        let png = Self.pngData(image)
        let tiff = image.tiffRepresentation
        var types: [NSPasteboard.PasteboardType] = []
        if png != nil { types.append(.png) }
        if tiff != nil { types.append(.tiff) }
        guard !types.isEmpty else {
            pasteboard.writeObjects([image])
            return
        }
        pasteboard.declareTypes(types, owner: nil)
        if let png { pasteboard.setData(png, forType: .png) }
        if let tiff { pasteboard.setData(tiff, forType: .tiff) }
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
                    .contentTransition(.symbolEffect(.replace))
                Text(label)
                    .font(Design.caption.weight(.medium))
                    .lineLimit(1)
                    .fixedSize()
                    .contentTransition(.opacity)
            }
            .foregroundStyle(hovering ? Design.ink : Design.inkSoft)
            .padding(.horizontal, Design.Space.s)
            .padding(.vertical, Design.Space.xxs + 1)
            .contentShape(Rectangle())
        }
        .buttonStyle(PressDipStyle())
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .animation(Design.spring, value: glyph)
        .animation(Design.spring, value: label)
        .accessibilityLabel(label)
    }
}

private struct ComposerTextView: NSViewRepresentable {
    @Binding var text: String
    let sendWithEnter: Bool
    @Binding var measuredHeight: CGFloat
    var onCommand: ((Selector) -> Bool)? = nil
    var focusToken: Int = 0
    var mentionPaths: Set<String> = []
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
            parent.restyleMentions(view)
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
        restyleMentions(view)
        remeasure(view)
    }

    func restyleMentions(_ view: NSTextView) {
        guard let manager = view.layoutManager else { return }
        let full = NSRange(location: 0, length: (view.string as NSString).length)
        manager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: full)
        guard !mentionPaths.isEmpty else { return }
        let accent = NSColor(Design.heatText)
        for range in Self.mentionRanges(in: view.string, paths: mentionPaths) {
            manager.addTemporaryAttribute(
                .foregroundColor, value: accent, forCharacterRange: range)
        }
    }

    static func mentionRanges(in text: String, paths: Set<String>) -> [NSRange] {
        guard let matcher = try? NSRegularExpression(pattern: "\\S+") else { return [] }
        let whole = text as NSString
        var ranges: [NSRange] = []
        for match in matcher.matches(in: text, range: NSRange(location: 0, length: whole.length)) {
            let token = whole.substring(with: match.range)
            if let (core, _) = PromptComposer.mentionCore(token), paths.contains(core) {
                ranges.append(match.range)
            }
        }
        return ranges
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
