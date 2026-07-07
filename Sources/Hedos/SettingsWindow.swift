import AppKit
import SwiftUI

@MainActor
final class SettingsWindowController {
    static let shared = SettingsWindowController()
    private var window: NSWindow?
    private weak var shell: ShellModel?

    func show(shell: ShellModel) {
        if window == nil {
            let window = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 920, height: 560),
                styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                backing: .buffered,
                defer: false)
            window.title = "Settings"
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.isReleasedWhenClosed = false
            window.isRestorable = false
            window.collectionBehavior = [.fullScreenNone]
            window.standardWindowButton(.zoomButton)?.isEnabled = false
            window.contentMinSize = NSSize(width: 760, height: 520)
            let hadSavedFrame =
                UserDefaults.standard.string(forKey: "NSWindow Frame HedosSettings") != nil
            window.setFrameAutosaveName("HedosSettings")
            if !hadSavedFrame {
                window.setContentSize(NSSize(width: 920, height: 560))
                window.center()
            }
            NotificationCenter.default.addObserver(
                self, selector: #selector(windowWillClose),
                name: NSWindow.willCloseNotification, object: window)
            self.window = window
        }
        if window?.contentViewController == nil || self.shell !== shell {
            let frame = window?.frame
            let hosting = NSHostingController(rootView: SettingsWindowRoot(shell: shell))
            hosting.sizingOptions = []
            window?.contentViewController = hosting
            if let frame {
                window?.setFrame(frame, display: false)
            }
            self.shell = shell
        }
        NSApp.activate()
        window?.makeKeyAndOrderFront(nil)
    }

    @objc private func windowWillClose() {
        window?.contentViewController = nil
        shell = nil
    }
}

private struct SettingsWindowRoot: View {
    let shell: ShellModel

    var body: some View {
        SettingsRoot(shell: shell)
            .tint(Design.ink)
    }
}
