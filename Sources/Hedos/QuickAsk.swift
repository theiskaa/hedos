import AppKit
import Carbon.HIToolbox
import HedosKernel
import SwiftUI

private final class KeyablePanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

@Observable
@MainActor
final class QuickAskModel {
    var draft = ""
    var answer = ""
    var isStreaming = false
    var notice: String?
    var focusToken = 0
    var visible = false
    var denyCount = 0
    private(set) var sessionID: String?
    private var task: Task<Void, Never>?

    func requestFocus() {
        focusToken += 1
    }

    func ask(kernel: Kernel, records: [ModelRecord]) {
        let question = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !isStreaming else { return }
        guard !question.isEmpty else {
            denyCount += 1
            return
        }
        guard Launcher.defaultChatModel(in: records) != nil else {
            notice = "No chat-capable model is ready."
            return
        }
        draft = ""
        answer = ""
        notice = nil
        isStreaming = true
        task = Task { [weak self] in
            guard let self else { return }
            defer { isStreaming = false }
            let preferred = await kernel.settings.defaultChatModelID()
            guard let record = Launcher.defaultChatModel(in: records, preferring: preferred)
            else { return }
            do {
                if sessionID == nil {
                    sessionID = try await kernel.createChatSession(modelID: record.id).id
                }
                guard let sessionID else { return }
                let stream = try await kernel.sendChat(sessionID: sessionID, text: question)
                for try await chunk in stream {
                    if case .text(let delta) = chunk {
                        answer += delta
                    }
                }
                _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
                Haptics.completion()
            } catch is CancellationError {
            } catch {
                notice = error.localizedDescription
            }
        }
    }

    func stop() {
        task?.cancel()
        isStreaming = false
    }

    func resetConversation() {
        stop()
        sessionID = nil
        answer = ""
        notice = nil
        draft = ""
    }
}

@MainActor
final class QuickAskController {
    static let shared = QuickAskController()
    weak var shell: ShellModel?
    private var panel: NSPanel?
    private let model = QuickAskModel()
    private var escapeMonitor: Any?

    func toggle() {
        if panel?.isVisible == true {
            hide()
        } else {
            show()
        }
    }

    private func materialize(_ shown: Bool) -> Animation {
        Design.reveal(
            shown,
            reduceMotion: NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
    }

    func hide() {
        if let escapeMonitor {
            NSEvent.removeMonitor(escapeMonitor)
            self.escapeMonitor = nil
        }
        withAnimation(materialize(false), completionCriteria: .logicallyComplete) {
            model.visible = false
        } completion: { [weak self] in
            self?.panel?.orderOut(nil)
        }
    }

    func show() {
        guard let shell else { return }
        if panel == nil {
            build(shell: shell)
        }
        guard let panel else { return }
        model.visible = false
        if let screen = NSScreen.main {
            let frame = screen.visibleFrame
            let size = panel.frame.size
            panel.setFrameOrigin(
                NSPoint(
                    x: frame.midX - size.width / 2,
                    y: frame.minY + frame.height * 0.72 - size.height / 2))
        }
        panel.orderFrontRegardless()
        panel.makeKey()
        model.requestFocus()
        withAnimation(materialize(true)) { model.visible = true }
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) {
            [weak self] event in
            if event.keyCode == UInt16(kVK_Escape), self?.panel?.isKeyWindow == true {
                self?.hide()
                return nil
            }
            return event
        }
    }

    func openInApp() {
        guard let shell else { return }
        hide()
        NSApp.activate(ignoringOtherApps: true)
        if let sessionID = model.sessionID {
            Task {
                await shell.refreshSessions()
                shell.selectChat(sessionID)
                shell.setMode(.chat)
            }
        }
        model.resetConversation()
    }

    private func build(shell: ShellModel) {
        let panel = KeyablePanel(
            contentRect: NSRect(origin: .zero, size: NSSize(width: 560, height: 96)),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false)
        panel.isFloatingPanel = true
        panel.level = .floating
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.becomesKeyOnlyIfNeeded = true
        let hosting = NSHostingController(
            rootView: QuickAskView(model: model, shell: shell))
        panel.contentViewController = hosting
        self.panel = panel
    }
}

private struct QuickAskView: View {
    @Bindable var model: QuickAskModel
    let shell: ShellModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            HStack(spacing: Design.Space.m) {
                Image(systemName: "sparkle")
                    .font(Design.glyphPrimary)
                    .foregroundStyle(Design.inkSoft)
                TextField(
                    "", text: $model.draft,
                    prompt: Text("Ask locally…").foregroundStyle(Design.inkFaint)
                )
                .textFieldStyle(.plain)
                .font(Design.heroBody)
                .foregroundStyle(Design.ink)
                .focused($focused)
                .onSubmit {
                    model.ask(kernel: shell.kernel, records: shell.library.records)
                }
                if model.isStreaming {
                    Button {
                        model.stop()
                    } label: {
                        Image(systemName: "stop.fill")
                            .font(Design.glyphInline)
                            .foregroundStyle(Design.inkSoft)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Stop")
                }
            }
            if let notice = model.notice {
                Text(notice)
                    .font(Design.label)
                    .foregroundStyle(Design.inkSoft)
            }
            if !model.answer.isEmpty {
                ScrollView {
                    Text(model.answer)
                        .font(Design.body)
                        .lineSpacing(Design.bodyLineSpacing)
                        .foregroundStyle(Design.ink)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 260)
                HStack {
                    Spacer()
                    Button("Open in Hedos →") {
                        QuickAskController.shared.openInApp()
                    }
                    .buttonStyle(QuietButtonStyle())
                }
            }
        }
        .padding(Design.Space.xxl)
        .frame(width: 560)
        .background(Design.paper, in: RoundedRectangle.soft(Design.Radius.surface))
        .overlay(
            RoundedRectangle.soft(Design.Radius.surface)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .shade(Design.Elevation.modal)
        .denyShake(
            on: model.denyCount, in: RoundedRectangle.soft(Design.Radius.surface), amplitude: 4)
        .scaleEffect(model.visible ? 1 : 0.96, anchor: .top)
        .opacity(model.visible ? 1 : 0)
        .onAppear { focused = true }
        .onChange(of: model.focusToken) { focused = true }
        .accessibilityIdentifier("quick-ask")
    }
}

