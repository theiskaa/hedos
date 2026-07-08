import AppKit
import HedosKernel
import SwiftUI

final class FullScreenPresentationProxy: NSObject, NSWindowDelegate {
    weak var base: NSWindowDelegate?

    override func responds(to selector: Selector!) -> Bool {
        super.responds(to: selector) || (base?.responds(to: selector) ?? false)
    }

    override func forwardingTarget(for selector: Selector!) -> Any? {
        base
    }

    func window(
        _ window: NSWindow,
        willUseFullScreenPresentationOptions proposedOptions: NSApplication.PresentationOptions
    ) -> NSApplication.PresentationOptions {
        [.fullScreen, .autoHideMenuBar, .autoHideDock, .autoHideToolbar]
    }
}

final class HedosAppDelegate: NSObject, NSApplicationDelegate {
    private var proxies: [ObjectIdentifier: FullScreenPresentationProxy] = [:]
    private var keyObserver: (any NSObjectProtocol)?

    func applicationDidFinishLaunching(_ notification: Notification) {
        keyObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self, let window = notification.object as? NSWindow,
                window.styleMask.contains(.fullSizeContentView) || window.toolbar != nil
            else { return }
            let key = ObjectIdentifier(window)
            if let proxy = self.proxies[key] {
                if window.delegate !== proxy {
                    proxy.base = window.delegate
                    window.delegate = proxy
                }
                return
            }
            let proxy = FullScreenPresentationProxy()
            proxy.base = window.delegate
            window.delegate = proxy
            self.proxies[key] = proxy
        }
        _ = NotificationCenter.default.addObserver(
            forName: NSWindow.willCloseNotification, object: nil, queue: .main
        ) { [weak self] notification in
            guard let self, let window = notification.object as? NSWindow else { return }
            self.proxies.removeValue(forKey: ObjectIdentifier(window))
        }
        if let iconURL = Bundle.module.url(forResource: "Resources/Hedos", withExtension: "icns")
            ?? Bundle.module.url(forResource: "Hedos", withExtension: "icns"),
            let icon = NSImage(contentsOf: iconURL)
        {
            NSApplication.shared.applicationIconImage = icon
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task {
            await QuickAskController.shared.shell?.kernel.stopWatching()
            await QuickAskController.shared.shell?.kernel.stopGateway()
            await SettingsModel.active?.flush()
            await MemoryGovernor.shared.suspendForQuit()
            await SidecarSupervisor.shared.terminateAll()
            await MainActor.run {
                sender.reply(toApplicationShouldTerminate: true)
            }
        }
        return .terminateLater
    }
}

@main
struct HedosApp: App {
    @NSApplicationDelegateAdaptor(HedosAppDelegate.self) private var delegate
    @Environment(\.openWindow) private var openWindow
    @State private var shell = ShellModel()

    init() {
        NSApplication.shared.setActivationPolicy(.regular)
    }

    var body: some Scene {
        WindowGroup("Hedos") {
            ShellView(shell: shell)
                .onAppear {
                    NSApp.activate(ignoringOtherApps: true)
                    QuickAskController.shared.shell = shell
                    MenuBarController.shared.shell = shell
                    HotkeyCenter.shared.onSummon = {
                        QuickAskController.shared.toggle()
                    }
                    SettingsWindowController.shared.prewarm(shell: shell)
                    FocusJanitor.shared.install()
                    DispatchQueue.main.async {
                        NSApp.keyWindow?.makeFirstResponder(nil)
                    }
                }
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About Hedos") {
                    openWindow(id: "about")
                }
            }
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") {
                    SettingsWindowController.shared.show(shell: shell)
                }
                .keyboardShortcut(",", modifiers: .command)
            }
            CommandGroup(after: .textEditing) {
                Button("Find Chats") {
                    shell.focusChatSearch()
                }
                .keyboardShortcut("f", modifiers: .command)
                Button("Next Chat") {
                    shell.selectAdjacentChat(1)
                }
                .keyboardShortcut("]", modifiers: [.command, .shift])
                Button("Previous Chat") {
                    shell.selectAdjacentChat(-1)
                }
                .keyboardShortcut("[", modifiers: [.command, .shift])
            }
            CommandGroup(replacing: .newItem) {
                Button("New Chat") {
                    shell.newChat()
                }
                .keyboardShortcut("n", modifiers: .command)
                Button("Import Chat…") {
                    let panel = NSOpenPanel()
                    panel.allowedContentTypes = [.json]
                    panel.allowsMultipleSelection = false
                    if panel.runModal() == .OK, let url = panel.url {
                        shell.importChat(from: url)
                    }
                }
            }
            CommandGroup(before: .toolbar) {
                ForEach(AppMode.allCases.filter { $0 != .settings }, id: \.self) { mode in
                    Button(Design.modeTitle(mode)) {
                        shell.setMode(mode)
                    }
                    .keyboardShortcut(
                        KeyEquivalent(Character("\(mode.ordinal)")), modifiers: .command)
                }
                Divider()
                Button(shell.sidebarCollapsed ? "Expand Sidebar" : "Collapse Sidebar") {
                    shell.setSidebarCollapsed(!shell.sidebarCollapsed)
                }
                .keyboardShortcut("s", modifiers: [.command, .option])
                Button("Toggle Full Screen") {
                    (NSApp.keyWindow ?? NSApp.mainWindow)?.toggleFullScreen(nil)
                }
                .keyboardShortcut("f", modifiers: [.command, .control])
                Divider()
            }
        }

        Window("About Hedos", id: "about") {
            AboutView()
        }
        .windowResizability(.contentSize)
    }
}

struct AboutView: View {
    var body: some View {
        VStack(spacing: 14) {
            HeptagonMark(size: 64, color: Design.ink)
            Text("Hedos")
                .font(Design.plaque(26, weight: .semibold))
            Text("A home for every local model.")
                .font(.callout)
                .foregroundStyle(Design.inkSoft)
            Text("kernel \(Kernel.version)")
                .font(Design.data(11))
                .foregroundStyle(Design.inkFaint)
            Text("ἕδος — a seat, an abode, a foundation.")
                .font(Design.plaque(12))
                .foregroundStyle(Design.inkFaint)
                .padding(.top, Design.Space.xxs)
        }
        .padding(Design.Space.pane)
        .frame(width: Design.Window.aboutWidth)
    }
}

@MainActor
final class FocusJanitor {
    static let shared = FocusJanitor()
    private var monitor: Any?

    func install() {
        guard monitor == nil else { return }
        monitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { event in
            guard let window = event.window,
                let textView = window.firstResponder as? NSTextView
            else { return event }
            let owner: NSView = (textView.delegate as? NSTextField) ?? textView
            var view = window.contentView?.superview?.hitTest(event.locationInWindow)
            while let current = view {
                if current === owner || current is NSTextView || current is NSTextField {
                    return event
                }
                view = current.superview
            }
            window.makeFirstResponder(nil)
            return event
        }
    }
}
