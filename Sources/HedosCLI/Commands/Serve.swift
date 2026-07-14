import ArgumentParser
import HedosKernel

struct Serve: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "serve",
        abstract: "Start the authenticated, OpenAI-compatible gateway on 127.0.0.1.")

    @OptionGroup var global: GlobalOptions

    @Option(name: [.customShort("p"), .long], help: "Port to listen on.")
    var port: Int?

    func run() async throws {
        let kernel = Session.kernel()
        let status = try await kernel.startGateway(portOverride: port)
        guard status.running, let boundPort = status.port else {
            throw CLIError("the gateway failed to start.")
        }
        let baseURL = GatewayDefaults.baseURL(port: boundPort)

        if global.json {
            try Out.json(ServeReport(running: true, port: boundPort, baseURL: baseURL))
        } else {
            Out.line("gateway listening on \(baseURL)")
            Out.err("authenticate with a bearer token from `hedos token new`. Press Ctrl-C to stop.")
        }

        await Signals.waitForInterrupt()
        await kernel.stopGateway()
        if !global.json { Out.err("gateway stopped.") }
    }
}

struct ServeReport: Encodable {
    let running: Bool
    let port: Int
    let baseURL: String
}
