import ArgumentParser
import Foundation
import HedosKernel

struct Chat: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "chat",
        abstract: "Hold a headless streaming chat with a model (reads turns from stdin).")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    @Option(name: .long, help: "System prompt override.")
    var system: String?

    @Option(name: .long, help: "Maximum tokens per reply.")
    var maxTokens: Int?

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf, capability: .chat)

        let interactive = isatty(fileno(stdin)) != 0
        if interactive && !global.json {
            Out.err("chatting with \(record.displayName) — press Ctrl-D to end.")
        }

        var history: [ChatMessage] = []
        while true {
            if interactive && !global.json { Out.err("› ") }
            guard let line = readLine(strippingNewline: true) else { break }
            let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty { continue }

            history.append(ChatMessage(role: .user, content: trimmed))
            var object: [String: JSONValue] = ["messages": .array(history.map(\.payloadValue))]
            if let maxTokens { object["max_tokens"] = .int(maxTokens) }

            let stream = try await kernel.invoke(
                record.id, .chat, payload: .object(object), systemPromptOverride: system)

            var reply = ""
            for try await chunk in stream {
                switch chunk {
                case .text(let piece):
                    reply += piece
                    if !global.json { Out.raw(piece) }
                case .status(let status):
                    if !global.json { Out.err(status) }
                default:
                    break
                }
            }
            if !reply.isEmpty {
                history.append(ChatMessage(role: .assistant, content: reply))
            }

            if global.json {
                try Out.json(ChatTurn(role: "assistant", content: reply))
            } else {
                Out.line("")
            }
        }
    }
}

struct ChatTurn: Encodable {
    let role: String
    let content: String
}
