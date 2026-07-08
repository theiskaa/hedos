import Foundation
import HedosKernel

let kernel = Kernel()
if CommandLine.arguments.contains("--discover") {
    _ = try await kernel.discover()
}

if CommandLine.arguments.contains("--sweep") {
    var perModelTimeout = Duration.seconds(120)
    if let index = CommandLine.arguments.firstIndex(of: "--timeout"),
        index + 1 < CommandLine.arguments.count,
        let seconds = Int(CommandLine.arguments[index + 1])
    {
        perModelTimeout = .seconds(seconds)
    }
    let includeImage = CommandLine.arguments.contains("--include-image")
    let results = await ShelfSweep.run(
        kernel, includeImage: includeImage, perModelTimeout: perModelTimeout)
    if results.isEmpty {
        print("shelf is empty — run with --discover to scan this machine")
    } else {
        print(SweepReport.render(results))
    }
    exit(results.contains { $0.status == .fail } ? 1 : 0)
}

let explanations = try await kernel.explainShelf()
if explanations.isEmpty {
    print("shelf is empty — run with --discover to scan this machine")
} else {
    print(ShelfReport.render(explanations))
}
