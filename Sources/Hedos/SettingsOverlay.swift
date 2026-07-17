import AppKit
import Carbon.HIToolbox
import SwiftUI

struct SettingsOverlay: ViewModifier {
    @Bindable var shell: ShellModel
    @State private var escapeMonitor: Any?

    func body(content: Content) -> some View {
        content
            .modalScrim(
                isPresented: shell.settingsOpen,
                handlesEscape: false,
                onDismiss: { shell.requestSettingsDismiss() }
            ) {
                SettingsRoot(
                    shell: shell,
                    dismissAttempts: shell.settingsDismissAttempts,
                    onClose: { shell.closeSettings() }
                )
                .frame(
                    maxWidth: Design.Sheet.settings.width,
                    maxHeight: Design.Sheet.settings.height)
            }
            .onChange(of: shell.settingsOpen) { _, open in
                if open {
                    installEscapeMonitor()
                } else {
                    removeEscapeMonitor()
                }
            }
            .onDisappear { removeEscapeMonitor() }
    }

    private func installEscapeMonitor() {
        guard escapeMonitor == nil else { return }
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.keyCode == UInt16(kVK_Escape),
                shell.settingsOpen,
                !shell.commandPaletteOpen,
                !KeyNavCoordinator.capturing,
                !(NSApp.keyWindow is NSPanel)
            else { return event }
            shell.requestSettingsDismiss()
            return nil
        }
    }

    private func removeEscapeMonitor() {
        if let escapeMonitor {
            NSEvent.removeMonitor(escapeMonitor)
        }
        escapeMonitor = nil
    }
}

extension View {
    func settingsOverlay(shell: ShellModel) -> some View {
        modifier(SettingsOverlay(shell: shell))
    }
}
