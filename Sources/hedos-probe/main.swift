import Foundation
import HedosKernel

let kernel = Kernel()
if CommandLine.arguments.contains("--discover") {
    _ = try await kernel.discover()
}

func stringArg(_ flag: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: flag),
        index + 1 < CommandLine.arguments.count
    else { return nil }
    return CommandLine.arguments[index + 1]
}

func intArg(_ flag: String, _ fallback: Int) -> Int {
    guard let value = stringArg(flag), let number = Int(value) else { return fallback }
    return number
}

if CommandLine.arguments.contains("--kv-check") {
    _ = try await kernel.discover()
    let shelf = try await kernel.shelf()
    let chat = shelf.filter { $0.capabilities.contains(.chat) }
    guard
        let model = chat.first(where: { $0.runtime.id?.rawValue == "mlx-swift" }) ?? chat.first
    else {
        print("no chat model")
        exit(1)
    }
    func ask(_ question: String) async throws -> String {
        let payload = JSONValue.object([
            "messages": .array([
                .object(["role": .string("user"), "content": .string(question)])
            ]),
            "max_tokens": .int(60), "temperature": .double(0.0),
        ])
        var out = ""
        for try await chunk in try await kernel.invoke(model.id, .chat, payload: payload) {
            if case .text(let piece) = chunk { out += piece }
        }
        return out.trimmingCharacters(in: .whitespacesAndNewlines)
    }
    print("Q1 (fresh):    ", try await ask("Name three primary colors."))
    print("Q2 (reuse):    ", try await ask("Name three primary colors."))
    print("Q3 (diverge):  ", try await ask("What is the capital of France? Answer in one word."))
    print("Q4 (reuse Q3): ", try await ask("What is the capital of France? Answer in one word."))
    exit(0)
}

if CommandLine.arguments.contains("--convo") {
    _ = try await kernel.discover()
    let shelf = try await kernel.shelf()
    let chat = shelf.filter { $0.capabilities.contains(.chat) }
    let requested = stringArg("--model")
    guard
        let model = requested.flatMap({ name in
            chat.first {
                $0.id == name || $0.name.localizedCaseInsensitiveContains(name)
                    || $0.displayName.localizedCaseInsensitiveContains(name)
            }
        }) ?? chat.first(where: { $0.runtime.id?.rawValue == "mlx-swift" }) ?? chat.first
    else {
        print("no chat model")
        exit(1)
    }
    let ttfts = try await ChatBenchmark.conversation(
        kernel: kernel, modelID: model.id, turns: intArg("--turns", 5))
    for (turn, ttft) in ttfts.enumerated() {
        print("turn \(turn + 1): ttft \(ttft) ms")
    }
    exit(0)
}

if CommandLine.arguments.contains("--bench") {
    _ = try await kernel.discover()
    let shelf = try await kernel.shelf()
    let chatModels = shelf.filter { $0.capabilities.contains(.chat) }
    let requested = stringArg("--model")
    let chosen: ModelRecord?
    if let requested {
        chosen =
            chatModels.first { $0.id == requested }
            ?? chatModels.first {
                $0.name.localizedCaseInsensitiveContains(requested)
                    || $0.displayName.localizedCaseInsensitiveContains(requested)
            }
    } else {
        chosen = chatModels.first
    }
    guard let model = chosen else {
        print("no chat-capable model matched — run with --discover and pass --model <id|name>")
        for record in chatModels {
            print("  \(record.id)  ·  \(record.displayName)  ·  \(record.runtime.id?.rawValue ?? "?")")
        }
        exit(1)
    }
    var prompt = ChatBenchmark.defaultPrompt
    let repeatCount = intArg("--prompt-repeat", 1)
    if repeatCount > 1 {
        prompt = String(repeating: ChatBenchmark.defaultPrompt + " ", count: repeatCount)
    }
    let report = try await ChatBenchmark.run(
        kernel: kernel, modelID: model.id, prompt: prompt,
        maxTokens: intArg("--max-tokens", 256),
        concurrency: intArg("--concurrency", 1),
        rounds: intArg("--rounds", 3))
    print(report.renderText())
    if let out = stringArg("--out") {
        let base = URL(fileURLWithPath: out)
        try? report.jsonString().write(
            to: base.appendingPathExtension("json"), atomically: true, encoding: .utf8)
        try? report.csv().write(
            to: base.appendingPathExtension("csv"), atomically: true, encoding: .utf8)
        print("wrote \(out).json and \(out).csv")
    }
    exit(0)
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
