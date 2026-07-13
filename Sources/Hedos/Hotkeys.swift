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
    @State private var denyCount = 0

    var body: some View {
        HStack(spacing: Design.Space.m) {
            Button {
                recording ? stopRecording() : startRecording()
            } label: {
                Group {
                    if recording {
                        ShimmerText(text: "Press keys…", font: Design.data(12), tracked: false)
                    } else if let hotkey {
                        Text(KeyDisplay.string(for: hotkey))
                            .font(Design.data(12))
                            .foregroundStyle(Design.ink)
                    } else {
                        Text("Record shortcut")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                    }
                }
                .padding(.horizontal, Design.Space.chipX)
                .padding(.vertical, Design.Space.xs + 1)
                .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.control))
                .overlay(
                    RoundedRectangle.soft(Design.Radius.control).strokeBorder(
                        recording
                            ? AnyShapeStyle(Design.ink.opacity(0.35))
                            : AnyShapeStyle(Design.line),
                        lineWidth: Design.hairlineWidth))
                .contentShape(RoundedRectangle.soft(Design.Radius.control))
            }
            .buttonStyle(PressDipStyle())
            .denyShake(on: denyCount, in: RoundedRectangle.soft(Design.Radius.control))
            .accessibilityLabel(
                recording ? "Recording shortcut, press keys or Escape to cancel"
                    : "Record global shortcut")
            Button("Clear") {
                onChange(nil)
            }
            .buttonStyle(QuietButtonStyle())
            .opacity(hotkey != nil && !recording ? 1 : 0)
            .disabled(hotkey == nil || recording)
            .allowsHitTesting(hotkey != nil && !recording)
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
                denyCount += 1
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

