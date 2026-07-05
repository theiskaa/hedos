import HedosKernel
import SwiftUI

@main
struct HedosApp: App {
    var body: some Scene {
        WindowGroup("Hedos") {
            ShelfPlaceholderView()
        }
    }
}

struct ShelfPlaceholderView: View {
    var body: some View {
        VStack(spacing: 8) {
            Text("Hedos")
                .font(.largeTitle.weight(.semibold))
            Text("A home for every local model.")
                .foregroundStyle(.secondary)
            Text("kernel \(Kernel.version)")
                .font(.caption.monospaced())
                .foregroundStyle(.tertiary)
        }
        .frame(minWidth: 480, minHeight: 320)
        .padding()
    }
}
