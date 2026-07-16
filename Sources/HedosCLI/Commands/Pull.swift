import ArgumentParser
import Foundation
import HedosKernel

struct Pull: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "pull",
        abstract: "Fetch a model from Ollama or Hugging Face onto the shelf.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model to fetch (an Ollama tag like gemma3:4b, or a Hugging Face org/repo).")
    var reference: String?

    @Option(
        name: .customLong("from"),
        help: "Where to fetch from: ollama or huggingface. Without it, org/repo goes to Hugging Face and everything else to Ollama (use user/model:latest for Ollama community models).")
    var from: String?

    func run() async throws {
        let kernel = Session.kernel()
        guard let reference else {
            try recommend()
            return
        }
        let provider = try Self.provider(for: reference, override: from)
        let plan: InstallPlan
        do {
            plan = try await kernel.installs.plan(provider: provider, reference: reference)
        } catch let error as InstallError {
            throw CLIError(error.localizedDescription)
        }
        if plan.requiresAuth {
            throw CLIError(
                "\(plan.reference) is gated. Set a token with HF_TOKEN or `huggingface-cli login`, then retry."
            )
        }
        if !global.json {
            let size = plan.totalBytes.map { " (~\(Self.gigabytes($0)))" } ?? ""
            Out.err("pulling \(plan.reference)\(size) into \(plan.destination)…")
        }
        let id: String
        do {
            id = try await kernel.installs.begin(plan)
        } catch let error as InstallError {
            throw CLIError(error.localizedDescription)
        }
        let interrupt = Task {
            await Signals.waitForInterrupt()
            guard !Task.isCancelled else { return }
            await kernel.installs.cancel(id)
        }
        var failure: String?
        var cancelled = false
        var lastBytes: Int64 = 0
        var progressLineActive = false
        var lastPlainProgressAt: ContinuousClock.Instant?
        for await event in await kernel.installs.events(id: id) {
            switch event {
            case .queued, .preparing, .done:
                break
            case .status(let message):
                if !global.json {
                    if progressLineActive {
                        clearProgressLine()
                        progressLineActive = false
                    }
                    Out.err(message)
                }
            case .progress(let progress):
                lastBytes = progress.bytesDownloaded
                if !global.json {
                    if progressToTTY {
                        renderProgress(progress)
                        progressLineActive = true
                    } else {
                        let now = ContinuousClock.Instant.now
                        let due =
                            lastPlainProgressAt.map { $0.duration(to: now) >= .seconds(1) }
                            ?? true
                        if due {
                            renderProgress(progress)
                            lastPlainProgressAt = now
                        }
                    }
                }
            case .failed(let message):
                failure = message
            case .cancelled:
                cancelled = true
            }
        }
        interrupt.cancel()
        Signals.restoreDefault()
        if !global.json { clearProgressLine() }
        if let failure {
            throw CLIError(failure)
        }
        if cancelled {
            throw CLIError(
                "cancelled — any substantial progress stays in the store; run the same pull to resume."
            )
        }
        let summary = try await kernel.discover()
        if global.json {
            try Out.json(
                PullReport(
                    pulled: plan.reference, provider: provider.rawValue,
                    bytes: plan.totalBytes ?? lastBytes, shelfCount: summary.totalCount))
        } else {
            Out.line("pulled \(plan.reference) — \(summary.headline)")
        }
    }

    static func provider(for reference: String, override: String? = nil) throws
        -> InstallProviderID
    {
        if let override {
            switch override.lowercased() {
            case "ollama": return .ollama
            case "huggingface", "hf": return .huggingface
            default:
                throw CLIError("--from accepts ollama or huggingface, not \(override).")
            }
        }
        if InstallReference.huggingFaceRepo(from: reference) != nil {
            return .huggingface
        }
        if InstallReference.ollamaTag(from: reference) != nil {
            return .ollama
        }
        throw CLIError(
            "\(reference) doesn't look like an Ollama tag (gemma3:4b), a Hugging Face org/repo, or a link to either."
        )
    }

    static func gigabytes(_ bytes: Int64) -> String {
        String(format: "%.1f GB", Double(bytes) / Double(1 << 30))
    }

    private var progressToTTY: Bool {
        isatty(fileno(stderr)) == 1
    }

    private func renderProgress(_ progress: InstallProgress) {
        let downloaded = DiscoverySummary.formatBytes(progress.bytesDownloaded)
        var line = downloaded
        if let total = progress.totalBytes {
            if let fraction = progress.fraction {
                let percent = Int(fraction * 100)
                line = "\(downloaded) / \(DiscoverySummary.formatBytes(total))  \(percent)%"
            } else if progress.totalIsPartial {
                line = "\(downloaded) / \(DiscoverySummary.formatBytes(total))+"
            }
        }
        if let file = progress.currentFile {
            line += "  \(file)"
        }
        if progressToTTY {
            FileHandle.standardError.write(Data(("\r\u{1B}[K" + line).utf8))
        } else {
            Out.err(line)
        }
    }

    private func clearProgressLine() {
        if progressToTTY {
            FileHandle.standardError.write(Data("\r\u{1B}[K".utf8))
        }
    }

    private func recommend() throws {
        let profile = HardwareProfile.current
        let picks = InstallCatalog.recommended(ramGB: profile.ramGB, providers: [.ollama])
        if global.json {
            try Out.json(
                RecommendReport(ramGB: profile.ramGB, recommended: picks.map(\.reference)))
        } else {
            Out.line("recommended for this Mac (\(profile.ramGB) GB):")
            for pick in picks {
                Out.line(
                    "  \(pick.reference)  ·  ~\(String(format: "%g", pick.sizeGB)) GB  ·  \(pick.blurb)")
            }
            Out.err("fetch one with `hedos pull <name>`.")
        }
    }
}

struct PullReport: Encodable {
    let pulled: String
    let provider: String
    let bytes: Int64
    let shelfCount: Int
}

struct RecommendReport: Encodable {
    let ramGB: Int
    let recommended: [String]
}
