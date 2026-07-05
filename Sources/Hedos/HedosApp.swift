import AppKit
import HedosKernel
import SwiftUI

final class HedosAppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        if let iconURL = Bundle.module.url(forResource: "Resources/Hedos", withExtension: "icns")
            ?? Bundle.module.url(forResource: "Hedos", withExtension: "icns"),
            let icon = NSImage(contentsOf: iconURL)
        {
            NSApplication.shared.applicationIconImage = icon
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task {
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

    init() {
        NSApplication.shared.setActivationPolicy(.regular)
    }

    var body: some Scene {
        WindowGroup("Hedos") {
            LibraryView()
                .onAppear {
                    NSApp.activate(ignoringOtherApps: true)
                }
        }
        .commands {
            CommandGroup(replacing: .appInfo) {
                Button("About Hedos") {
                    openWindow(id: "about")
                }
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
            HeptagonMark(size: 64, color: .primary.opacity(0.9))
            Text("Hedos")
                .font(Design.plaque(26, weight: .semibold))
            Text("A home for every local model.")
                .font(.callout)
                .foregroundStyle(.secondary)
            Text("kernel \(Kernel.version)")
                .font(Design.data(11))
                .foregroundStyle(.tertiary)
            Text("ἕδος — a seat, an abode, a foundation.")
                .font(Design.plaque(12))
                .foregroundStyle(.tertiary)
                .padding(.top, 2)
        }
        .padding(36)
        .frame(width: 300)
    }
}
