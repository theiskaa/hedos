import ArgumentParser
import HedosKernel

struct Run: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "run",
        abstract: "Stream a single completion from a model.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    @Argument(help: "The prompt.")
    var prompt: String

    @Option(name: .long, help: "System prompt override.")
    var system: String?

    @Option(name: .long, help: "Maximum tokens to generate.")
    var maxTokens: Int?

    @Option(name: .long, help: "Sampling temperature.")
    var temperature: Double?

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf, capability: .chat)

        var object: [String: JSONValue] = [
            "messages": .array([
                .object(["role": .string("user"), "content": .string(prompt)])
            ])
        ]
        if let maxTokens { object["max_tokens"] = .int(maxTokens) }
        if let temperature { object["temperature"] = .double(temperature) }

        let stream = try await kernel.invoke(
            record.id, .chat, payload: .object(object), systemPromptOverride: system)

        var text = ""
        var stats: GenerationStats?
        for try await chunk in stream {
            switch chunk {
            case .text(let piece):
                text += piece
                if !global.json { Out.raw(piece) }
            case .status(let status):
                if !global.json { Out.err(status) }
            case .done(let generationStats):
                stats = generationStats
            default:
                break
            }
        }

        if global.json {
            try Out.json(RunReport(model: record.id, text: text, stats: stats))
        } else {
            Out.line("")
        }
    }
}

struct RunReport: Encodable {
    let model: String
    let text: String
    let stats: GenerationStats?
}
