import ArgumentParser
import HedosKernel

struct Unload: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "unload",
        abstract: "Evict a model from residency through the memory governor.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf)

        let daemonHeld = (await kernel.residentModels()).contains {
            $0.origin == .ollama && $0.name == record.name
        }
        await kernel.governor.residency.unloadNow(record.id)
        let resident = await kernel.governor.isResident(record.id)

        if global.json {
            try Out.json(ResidencyReport(model: record.id, resident: resident || daemonHeld))
        } else if daemonHeld {
            Out.line("\(record.displayName) is loaded by Ollama — its daemon controls keep-alive, not hedos")
        } else if resident {
            Out.line("\(record.displayName) is still resident")
        } else {
            Out.line("unloaded \(record.displayName)")
        }
    }
}
