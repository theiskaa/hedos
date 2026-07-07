import AppKit
import Carbon.HIToolbox
import HedosKernel
import SwiftUI

@MainActor
final class HotkeyCenter {
    static let shared = HotkeyCenter()
    var onSummon: (() -> Void)?
    private var hotKeyRef: EventHotKeyRef?
    private var handlerRef: EventHandlerRef?

    func apply(_ hotkey: QuickAskHotkey?) {
        if let hotKeyRef {
            UnregisterEventHotKey(hotKeyRef)
            self.hotKeyRef = nil
        }
        guard let hotkey else { return }
        installHandlerIfNeeded()
        var ref: EventHotKeyRef?
        let id = EventHotKeyID(signature: OSType(0x4844_4F53), id: 1)
        RegisterEventHotKey(
            UInt32(hotkey.keyCode), UInt32(hotkey.modifiers), id,
            GetApplicationEventTarget(), 0, &ref)
        hotKeyRef = ref
    }

    private func installHandlerIfNeeded() {
        guard handlerRef == nil else { return }
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData else { return noErr }
                let center = Unmanaged<HotkeyCenter>.fromOpaque(userData)
                    .takeUnretainedValue()
                DispatchQueue.main.async {
                    center.onSummon?()
                }
                return noErr
            },
            1, &spec, Unmanaged.passUnretained(self).toOpaque(), &handlerRef)
    }
}

enum KeyDisplay {
    static func string(for hotkey: QuickAskHotkey) -> String {
        var parts = ""
        if hotkey.modifiers & controlKey != 0 { parts += "⌃" }
        if hotkey.modifiers & optionKey != 0 { parts += "⌥" }
        if hotkey.modifiers & shiftKey != 0 { parts += "⇧" }
        if hotkey.modifiers & cmdKey != 0 { parts += "⌘" }
        return parts + keyName(hotkey.keyCode)
    }

    static func carbonModifiers(from flags: NSEvent.ModifierFlags) -> Int {
        var carbon = 0
        if flags.contains(.command) { carbon |= cmdKey }
        if flags.contains(.shift) { carbon |= shiftKey }
        if flags.contains(.option) { carbon |= optionKey }
        if flags.contains(.control) { carbon |= controlKey }
        return carbon
    }

    static func keyName(_ keyCode: Int) -> String {
        switch keyCode {
        case kVK_Space: return "Space"
        case kVK_Return: return "↩"
        case kVK_Tab: return "⇥"
        case kVK_Delete: return "⌫"
        case kVK_Escape: return "⎋"
        case kVK_LeftArrow: return "←"
        case kVK_RightArrow: return "→"
        case kVK_UpArrow: return "↑"
        case kVK_DownArrow: return "↓"
        case kVK_F1...kVK_F12: return "F\(fNumber(keyCode))"
        default: break
        }
        guard
            let source = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
            let layoutPointer = TISGetInputSourceProperty(
                source, kTISPropertyUnicodeKeyLayoutData)
        else { return "key \(keyCode)" }
        let data = Unmanaged<CFData>.fromOpaque(layoutPointer).takeUnretainedValue() as Data
        var deadKeys: UInt32 = 0
        var characters = [UniChar](repeating: 0, count: 4)
        var length = 0
        let status = data.withUnsafeBytes { raw -> OSStatus in
            guard let layout = raw.bindMemory(to: UCKeyboardLayout.self).baseAddress else {
                return OSStatus(paramErr)
            }
            return UCKeyTranslate(
                layout, UInt16(keyCode), UInt16(kUCKeyActionDisplay), 0,
                UInt32(LMGetKbdType()), UInt32(kUCKeyTranslateNoDeadKeysBit),
                &deadKeys, characters.count, &length, &characters)
        }
        guard status == noErr, length > 0 else { return "key \(keyCode)" }
        return String(utf16CodeUnits: characters, count: length).uppercased()
    }

    private static func fNumber(_ keyCode: Int) -> Int {
        switch keyCode {
        case kVK_F1: 1
        case kVK_F2: 2
        case kVK_F3: 3
        case kVK_F4: 4
        case kVK_F5: 5
        case kVK_F6: 6
        case kVK_F7: 7
        case kVK_F8: 8
        case kVK_F9: 9
        case kVK_F10: 10
        case kVK_F11: 11
        default: 12
        }
    }
}

struct HotkeyRecorder: View {
    let hotkey: QuickAskHotkey?
    let onChange: (QuickAskHotkey?) -> Void
    @State private var recording = false
    @State private var monitor: Any?

    var body: some View {
        HStack(spacing: Design.Space.m) {
            Button {
                recording ? stopRecording() : startRecording()
            } label: {
                Text(recording ? "Press keys…" : hotkey.map(KeyDisplay.string(for:)) ?? "Record shortcut")
                    .font(hotkey == nil && !recording ? Design.label : Design.data(12))
                    .foregroundStyle(
                        recording
                            ? Design.inkSoft : hotkey == nil ? Design.inkFaint : Design.ink)
                    .padding(.horizontal, Design.Space.chipX)
                    .padding(.vertical, Design.Space.xs + 1)
                    .background(Design.surface, in: Capsule())
                    .overlay(
                        Capsule().strokeBorder(
                            recording
                                ? AnyShapeStyle(Design.ink.opacity(0.35))
                                : AnyShapeStyle(Design.line),
                            lineWidth: Design.hairlineWidth))
                    .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(
                recording ? "Recording shortcut, press keys or Escape to cancel"
                    : "Record global shortcut")
            if hotkey != nil && !recording {
                Button("Clear") {
                    onChange(nil)
                }
                .buttonStyle(QuietButtonStyle())
            }
        }
        .onDisappear { stopRecording() }
    }

    private func startRecording() {
        recording = true
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            defer { stopRecording() }
            if event.keyCode == UInt16(kVK_Escape) {
                return nil
            }
            if event.keyCode == UInt16(kVK_Delete) {
                onChange(nil)
                return nil
            }
            let modifiers = KeyDisplay.carbonModifiers(from: event.modifierFlags)
            guard modifiers & (cmdKey | optionKey | controlKey) != 0 else {
                return nil
            }
            onChange(QuickAskHotkey(keyCode: Int(event.keyCode), modifiers: modifiers))
            return nil
        }
    }

    private func stopRecording() {
        recording = false
        if let monitor {
            NSEvent.removeMonitor(monitor)
            self.monitor = nil
        }
    }
}

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
    private(set) var sessionID: String?
    private var task: Task<Void, Never>?

    func ask(kernel: Kernel, records: [ModelRecord]) {
        let question = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !question.isEmpty, !isStreaming else { return }
        guard let record = Launcher.defaultChatModel(in: records) else {
            notice = "No chat-capable model is ready."
            return
        }
        draft = ""
        answer = ""
        notice = nil
        isStreaming = true
        task = Task { [weak self] in
            guard let self else { return }
            do {
                if sessionID == nil {
                    sessionID = try await kernel.chats.createSession(modelID: record.id).id
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
            isStreaming = false
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

    func hide() {
        panel?.orderOut(nil)
        if let escapeMonitor {
            NSEvent.removeMonitor(escapeMonitor)
            self.escapeMonitor = nil
        }
    }

    func show() {
        guard let shell else { return }
        if panel == nil {
            build(shell: shell)
        }
        guard let panel else { return }
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
        .background(Design.paper, in: RoundedRectangle(cornerRadius: Design.Radius.surface))
        .overlay(
            RoundedRectangle(cornerRadius: Design.Radius.surface)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .shadow(color: Design.shadowColor.opacity(0.30), radius: 40, x: 0, y: 18)
        .onAppear { focused = true }
        .accessibilityIdentifier("quick-ask")
    }
}

@MainActor
final class MenuBarController {
    static let shared = MenuBarController()
    weak var shell: ShellModel?
    private var item: NSStatusItem?
    private var activityTask: Task<Void, Never>?

    func apply(_ enabled: Bool) {
        if enabled {
            guard item == nil else { return }
            let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
            item.button?.image = Self.icon(active: false)
            item.menu = buildMenu()
            self.item = item
            watchActivity()
        } else {
            if let item {
                NSStatusBar.system.removeStatusItem(item)
            }
            item = nil
            activityTask?.cancel()
            activityTask = nil
        }
    }

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        let show = NSMenuItem(
            title: "Show Hedos", action: #selector(showApp), keyEquivalent: "")
        show.target = self
        menu.addItem(show)
        let ask = NSMenuItem(
            title: "Quick Ask", action: #selector(quickAsk), keyEquivalent: "")
        ask.target = self
        menu.addItem(ask)
        menu.addItem(.separator())
        let quit = NSMenuItem(
            title: "Quit Hedos", action: #selector(NSApplication.terminate(_:)),
            keyEquivalent: "q")
        menu.addItem(quit)
        return menu
    }

    @objc private func showApp() {
        NSApp.activate(ignoringOtherApps: true)
        NSApp.windows.first { $0.styleMask.contains(.fullSizeContentView) && !($0 is NSPanel) }?
            .makeKeyAndOrderFront(nil)
    }

    @objc private func quickAsk() {
        QuickAskController.shared.toggle()
    }

    private func watchActivity() {
        activityTask?.cancel()
        activityTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, let shell = self.shell else { return }
                let active = await shell.kernel.activeJobs().count > 0
                self.item?.button?.image = Self.icon(active: active)
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private static func icon(active: Bool) -> NSImage {
        let size = NSSize(width: 18, height: 18)
        let image = NSImage(size: size, flipped: false) { rect in
            let path = NSBezierPath()
            let center = NSPoint(x: rect.midX, y: rect.midY)
            let radius = min(rect.width, rect.height) * 0.42
            for index in 0..<7 {
                let angle = (Double(index) * 2 * .pi / 7) + .pi / 2
                let point = NSPoint(
                    x: center.x + radius * cos(angle),
                    y: center.y + radius * sin(angle))
                if index == 0 {
                    path.move(to: point)
                } else {
                    path.line(to: point)
                }
            }
            path.close()
            NSColor.black.setFill()
            path.fill()
            if active {
                let dot = NSBezierPath(
                    ovalIn: NSRect(x: rect.maxX - 5, y: rect.minY, width: 5, height: 5))
                dot.fill()
            }
            return true
        }
        image.isTemplate = true
        return image
    }
}

enum Haptics {
    @MainActor
    static func completion() {
        NSHapticFeedbackManager.defaultPerformer.perform(
            .levelChange, performanceTime: .drawCompleted)
    }
}
