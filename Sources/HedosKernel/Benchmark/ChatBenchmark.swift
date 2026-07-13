import Darwin
import Foundation

public struct BenchmarkRun: Codable, Sendable {
    public let ttftMs: Int
    public let totalMs: Int
    public let completionTokens: Int
    public let chars: Int
    public let tokensPerSec: Double
    public let charsPerSec: Double
    public let loadMs: Int?
    public let producedText: Bool
}

public struct BenchmarkRound: Codable, Sendable {
    public let index: Int
    public let wallMs: Int
    public let runs: [BenchmarkRun]
    public let serializedTokensPerSec: Double
}

public struct BenchmarkReport: Codable, Sendable {
    public let model: String
    public let runtime: String
    public let concurrency: Int
    public let maxTokens: Int
    public let rounds: [BenchmarkRound]
    public let warmMeasured: Bool
    public let ttftMsMedian: Int
    public let tokensPerSecMedian: Double
    public let charsPerSecMedian: Double
    public let serializedTokensPerSecMedian: Double
    public let peakResidentMB: Int
    public let firstLoadMs: Int?
}

private final class PeakBox: @unchecked Sendable {
    var mb = 0
}

public enum ChatBenchmark {
    public static let defaultPrompt =
        "Explain how photosynthesis works, in a few clear sentences."

    public static func run(
        kernel: Kernel, modelID: String, prompt: String = defaultPrompt,
        maxTokens: Int = 256, concurrency: Int = 1, rounds: Int = 3
    ) async throws -> BenchmarkReport {
        let record = try await kernel.registry.get(id: modelID)
        let runtime = record?.runtime.id?.rawValue ?? "unknown"
        let lanes = max(1, concurrency)

        let peak = PeakBox()
        let sampler = Task {
            while !Task.isCancelled {
                peak.mb = max(peak.mb, residentMB())
                try? await Task.sleep(for: .milliseconds(50))
            }
        }

        var roundResults: [BenchmarkRound] = []
        var firstLoadMs: Int?

        for roundIndex in 0..<max(1, rounds) {
            let clock = ContinuousClock()
            let roundStart = clock.now
            let runs = try await withThrowingTaskGroup(of: BenchmarkRun.self) { group in
                for lane in 0..<lanes {
                    let unique = "\(prompt) [r\(roundIndex)l\(lane)]"
                    group.addTask {
                        try await measureOne(
                            kernel: kernel, modelID: modelID, prompt: unique,
                            maxTokens: maxTokens)
                    }
                }
                var collected: [BenchmarkRun] = []
                for try await run in group { collected.append(run) }
                return collected
            }
            let wallMs = milliseconds(clock.now - roundStart)
            let totalTokens = runs.reduce(0) { $0 + $1.completionTokens }
            let serialized = wallMs > 0 ? Double(totalTokens) / (Double(wallMs) / 1000.0) : 0
            roundResults.append(
                BenchmarkRound(
                    index: roundIndex, wallMs: wallMs, runs: runs,
                    serializedTokensPerSec: serialized))
            if roundIndex == 0 { firstLoadMs = runs.compactMap(\.loadMs).max() }
        }

        sampler.cancel()
        _ = await sampler.value

        let warmMeasured = roundResults.count > 1
        let sampleRounds = warmMeasured ? Array(roundResults.dropFirst()) : roundResults
        let sampleRuns = sampleRounds.flatMap(\.runs).filter(\.producedText)
        return BenchmarkReport(
            model: modelID,
            runtime: runtime,
            concurrency: lanes,
            maxTokens: maxTokens,
            rounds: roundResults,
            warmMeasured: warmMeasured,
            ttftMsMedian: Int(median(sampleRuns.map { Double($0.ttftMs) })),
            tokensPerSecMedian: median(sampleRuns.map(\.tokensPerSec)),
            charsPerSecMedian: median(sampleRuns.map(\.charsPerSec)),
            serializedTokensPerSecMedian: median(sampleRounds.map(\.serializedTokensPerSec)),
            peakResidentMB: peak.mb,
            firstLoadMs: firstLoadMs)
    }

    public static func conversation(
        kernel: Kernel, modelID: String, turns: Int, maxTokens: Int = 120
    ) async throws -> [Int] {
        let questions = [
            "Explain how photosynthesis works.",
            "Now explain it more simply.",
            "Give a short analogy for it.",
            "What is the key molecule involved?",
            "Summarize the whole thing in one sentence.",
        ]
        var messages: [JSONValue] = []
        var ttfts: [Int] = []
        for turn in 0..<max(1, turns) {
            messages.append(
                .object([
                    "role": .string("user"),
                    "content": .string(questions[turn % questions.count]),
                ]))
            let payload = JSONValue.object([
                "messages": .array(messages),
                "max_tokens": .int(maxTokens),
                "temperature": .double(0.0),
            ])
            let clock = ContinuousClock()
            let start = clock.now
            var firstToken: ContinuousClock.Instant?
            var reply = ""
            for try await chunk in try await kernel.invoke(modelID, .chat, payload: payload) {
                if case .text(let piece) = chunk {
                    if firstToken == nil { firstToken = clock.now }
                    reply += piece
                }
            }
            ttfts.append(firstToken.map { milliseconds($0 - start) } ?? milliseconds(clock.now - start))
            guard !reply.isEmpty else { break }
            messages.append(
                .object(["role": .string("assistant"), "content": .string(reply)]))
        }
        return ttfts
    }

    private static func measureOne(
        kernel: Kernel, modelID: String, prompt: String, maxTokens: Int
    ) async throws -> BenchmarkRun {
        let payload = JSONValue.object([
            "messages": .array([
                .object(["role": .string("user"), "content": .string(prompt)])
            ]),
            "max_tokens": .int(maxTokens),
            "temperature": .double(0.0),
        ])
        let clock = ContinuousClock()
        let start = clock.now
        var firstToken: ContinuousClock.Instant?
        var chars = 0
        var completionTokens = 0
        var loadMs: Int?
        let stream = try await kernel.invoke(modelID, .chat, payload: payload)
        for try await chunk in stream {
            switch chunk {
            case .text(let piece):
                if firstToken == nil { firstToken = clock.now }
                chars += piece.count
            case .done(let stats):
                if let stats {
                    if let tokens = stats.completionTokens { completionTokens = tokens }
                    loadMs = stats.loadMs
                }
            default:
                break
            }
        }
        let end = clock.now
        let producedText = firstToken != nil
        let totalMs = milliseconds(end - start)
        let ttftMs = firstToken.map { milliseconds($0 - start) } ?? totalMs
        let totalSeconds = seconds(end - start)
        let ttftSeconds = firstToken.map { seconds($0 - start) } ?? totalSeconds
        let decodeSeconds = totalSeconds - ttftSeconds
        let decodeRate =
            producedText && completionTokens > 1 && decodeSeconds > 0
            ? Double(completionTokens - 1) / decodeSeconds : 0
        let charsRate = producedText && totalSeconds > 0 ? Double(chars) / totalSeconds : 0
        return BenchmarkRun(
            ttftMs: ttftMs, totalMs: totalMs, completionTokens: completionTokens, chars: chars,
            tokensPerSec: decodeRate, charsPerSec: charsRate, loadMs: loadMs,
            producedText: producedText)
    }

    private static func milliseconds(_ duration: Duration) -> Int {
        Int(duration / .milliseconds(1))
    }

    private static func seconds(_ duration: Duration) -> Double {
        let parts = duration.components
        return Double(parts.seconds) + Double(parts.attoseconds) / 1e18
    }

    private static func median(_ values: [Double]) -> Double {
        guard !values.isEmpty else { return 0 }
        let sorted = values.sorted()
        let mid = sorted.count / 2
        return sorted.count.isMultiple(of: 2) ? (sorted[mid - 1] + sorted[mid]) / 2 : sorted[mid]
    }

    private static func residentMB() -> Int {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(
            MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { rebound in
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), rebound, &count)
            }
        }
        guard result == KERN_SUCCESS else { return 0 }
        return Int(info.resident_size) / (1 << 20)
    }
}

extension BenchmarkReport {
    public func renderText() -> String {
        let label = warmMeasured ? "warm median" : "median (single round, includes load)"
        var lines: [String] = []
        lines.append("model      \(model)")
        lines.append("runtime    \(runtime)")
        lines.append("concurrency \(concurrency)   maxTokens \(maxTokens)   rounds \(rounds.count)")
        if let firstLoadMs {
            lines.append("first load \(firstLoadMs) ms")
        }
        let ttftLabel = concurrency > 1 ? "\(label), includes serialization queue wait" : label
        lines.append("ttft       \(ttftMsMedian) ms (\(ttftLabel))")
        lines.append("decode     \(String(format: "%.1f", tokensPerSecMedian)) tok/s (\(label))")
        lines.append("chars      \(String(format: "%.1f", charsPerSecMedian)) chars/s (\(label))")
        if concurrency > 1 {
            lines.append(
                "serialized \(String(format: "%.1f", serializedTokensPerSecMedian)) tok/s "
                    + "total over \(concurrency) (engine serializes generations)")
        }
        lines.append("peak rss   \(peakResidentMB) MB")
        return lines.joined(separator: "\n")
    }

    public func csv() -> String {
        var out =
            "round,run,produced_text,ttft_ms,total_ms,completion_tokens,chars,tokens_per_s,chars_per_s\n"
        for round in rounds {
            for (index, run) in round.runs.enumerated() {
                out +=
                    "\(round.index),\(index),\(run.producedText),\(run.ttftMs),\(run.totalMs),"
                    + "\(run.completionTokens),\(run.chars),"
                    + "\(String(format: "%.2f", run.tokensPerSec)),"
                    + "\(String(format: "%.2f", run.charsPerSec))\n"
            }
        }
        return out
    }

    public func jsonString() -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        guard let data = try? encoder.encode(self),
            let text = String(data: data, encoding: .utf8)
        else { return "{}" }
        return text
    }
}
