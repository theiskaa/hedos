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
        await printEnvironmentalGaps()
    }
    exit(results.contains { $0.status == .fail } ? 1 : 0)
}

func parsedTimeout(_ fallback: Duration) -> Duration {
    if let index = CommandLine.arguments.firstIndex(of: "--timeout"),
        index + 1 < CommandLine.arguments.count,
        let seconds = Int(CommandLine.arguments[index + 1])
    {
        return .seconds(seconds)
    }
    return fallback
}

func printEnvironmentalGaps() async {
    let shelf = (try? await kernel.shelf()) ?? []
    for gap in EnvironmentalGaps.open(shelf: shelf) {
        print("environmental gap: \(gap)")
    }
}

if CommandLine.arguments.contains("--conformance") {
    let cells = await ConformanceMatrix.run(
        kernel, perCheckTimeout: parsedTimeout(.seconds(60)))
    if cells.isEmpty {
        print("shelf is empty — run with --discover to scan this machine")
        exit(0)
    }
    print(ConformanceReport.render(cells))
    await printEnvironmentalGaps()

    let failed = cells.contains { $0.status == .fail }
    if CommandLine.arguments.contains("--record-baseline") {
        do {
            try ConformanceBaseline.from(cells).save(kernelDirectory: kernel.directory)
            let passing = cells.filter { $0.status == .pass }.count
            print("recorded a baseline of \(passing) passing cells")
            exit(0)
        } catch {
            print("failed to write the baseline: \(error)")
            exit(1)
        }
    }
    let regressions =
        ConformanceBaseline.load(kernelDirectory: kernel.directory)?.regressions(in: cells) ?? []
    for regression in regressions {
        print(
            "regression: \(regression.model) · \(regression.conformanceClass.rawValue) · "
            + "\(regression.contract.rawValue) was passing and now \(regression.status.rawValue)")
    }
    exit(failed || !regressions.isEmpty ? 1 : 0)
}

let explanations = try await kernel.explainShelf()
if explanations.isEmpty {
    print("shelf is empty — run with --discover to scan this machine")
} else {
    print(ShelfReport.render(explanations))
}
