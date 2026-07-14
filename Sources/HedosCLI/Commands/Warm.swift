import ArgumentParser
import HedosKernel

struct Warm: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "warm",
        abstract: "Load a model into residency through the memory governor.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf)

        if record.capabilities.contains(.chat) || record.capabilities.contains(.complete) {
            let capability: Capability = record.capabilities.contains(.chat) ? .chat : .complete
            let payload: JSONValue = .object([
                "messages": .array([.object(["role": .string("user"), "content": .string("hi")])]),
                "max_tokens": .int(1),
            ])
            for try await _ in try await kernel.invoke(record.id, capability, payload: payload) {}
        } else if record.capabilities.contains(.speak) {
            let voice = (try? await kernel.voices(for: record.id))?.first ?? "default"
            let payload: JSONValue = .object(["text": .string("."), "voice": .string(voice)])
            for try await _ in try await kernel.invoke(record.id, .speak, payload: payload) {}
        } else {
            throw CLIError(
                "warming \(record.displayName) isn't supported — run it directly to load it.")
        }

        let daemonHeld = (await kernel.residentModels()).contains {
            $0.origin == .ollama && $0.name == record.name
        }
        let resident = await kernel.governor.isResident(record.id)
        if global.json {
            try Out.json(ResidencyReport(model: record.id, resident: resident || daemonHeld))
        } else if daemonHeld {
            Out.line("warmed \(record.displayName) — Ollama keeps it loaded")
        } else {
            Out.line(
                "loaded \(record.displayName) — in-process residency ends when hedos exits; "
                + "warm it inside `hedos serve` to keep it")
        }
    }
}

struct ResidencyReport: Encodable {
    let model: String
    let resident: Bool
}
