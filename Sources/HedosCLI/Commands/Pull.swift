import ArgumentParser
import Foundation
import HedosKernel

struct Pull: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "pull",
        abstract: "Fetch a recommended model into hedos from the terminal.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model to fetch (an Ollama tag like gemma3:4b, or a Hugging Face repo).")
    var suggestion: String?

    func run() async throws {
        let kernel = Session.kernel()
        guard let suggestion else {
            try recommend()
            return
        }

        if suggestion.contains("/") && !suggestion.contains(":") {
            throw CLIError(
                "\(suggestion) looks like a Hugging Face repo. Download it with `huggingface-cli "
                + "download \(suggestion)`, then run `hedos scan`.")
        }

        if !global.json { Out.err("pulling \(suggestion) with ollama…") }
        let status = runOllamaPull(suggestion)
        guard status == 0 else {
            if status == 127 {
                throw CLIError("`ollama` is not installed or not on PATH — install it, then retry.")
            }
            throw CLIError("`ollama pull \(suggestion)` failed (exit \(status)).")
        }

        let summary = try await kernel.discover()
        if global.json {
            try Out.json(PullReport(pulled: suggestion, shelfCount: summary.totalCount))
        } else {
            Out.line("pulled \(suggestion) — \(summary.headline)")
        }
    }

    private func recommend() throws {
        let profile = HardwareProfile.current
        let picks = Recommendation.forRAM(profile.ramGB)
        if global.json {
            try Out.json(RecommendReport(ramGB: profile.ramGB, recommended: picks.map(\.name)))
        } else {
            Out.line("recommended for this Mac (\(profile.ramGB) GB):")
            for pick in picks { Out.line("  \(pick.name)  ·  ~\(pick.sizeGB) GB  ·  \(pick.blurb)") }
            Out.err("fetch one with `hedos pull <name>`.")
        }
    }

    private func runOllamaPull(_ name: String) -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["ollama", "pull", name]
        process.standardOutput = FileHandle.standardError
        process.standardError = FileHandle.standardError
        do {
            try process.run()
        } catch {
            return 127
        }
        process.waitUntilExit()
        return process.terminationStatus
    }
}

struct Recommendation {
    let name: String
    let sizeGB: Int
    let blurb: String

    static let catalog: [Recommendation] = [
        Recommendation(name: "gemma3:1b", sizeGB: 1, blurb: "tiny, always fits"),
        Recommendation(name: "llama3.2:3b", sizeGB: 2, blurb: "fast general chat"),
        Recommendation(name: "gemma3:4b", sizeGB: 3, blurb: "balanced quality"),
        Recommendation(name: "qwen2.5-coder:7b", sizeGB: 5, blurb: "coding"),
        Recommendation(name: "gemma3:12b", sizeGB: 8, blurb: "stronger reasoning"),
        Recommendation(name: "gemma3:27b", sizeGB: 17, blurb: "high quality"),
        Recommendation(name: "llama3.3:70b", sizeGB: 43, blurb: "frontier, needs room"),
    ]

    static func forRAM(_ ramGB: Int) -> [Recommendation] {
        let ceiling = Double(ramGB) * 0.6
        let fitting = catalog.filter { Double($0.sizeGB) <= ceiling }
        return Array((fitting.isEmpty ? [catalog[0]] : fitting).suffix(3))
    }
}

struct HardwareProfile {
    let ramGB: Int

    static var current: HardwareProfile {
        HardwareProfile(ramGB: Int(ProcessInfo.processInfo.physicalMemory / 1_073_741_824))
    }
}

struct PullReport: Encodable {
    let pulled: String
    let shelfCount: Int
}

struct RecommendReport: Encodable {
    let ramGB: Int
    let recommended: [String]
}
