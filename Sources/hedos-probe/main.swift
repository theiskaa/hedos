import Foundation
import HedosKernel

let kernel = Kernel()
if CommandLine.arguments.contains("--discover") {
    _ = try await kernel.discover()
}
let explanations = try await kernel.explainShelf()
if explanations.isEmpty {
    print("shelf is empty — run with --discover to scan this machine")
} else {
    print(ShelfReport.render(explanations))
}
