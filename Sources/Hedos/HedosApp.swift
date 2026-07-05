import AppKit
import HedosKernel
import SwiftUI

@main
struct HedosApp: App {
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
    }
}
