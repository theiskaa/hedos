import ArgumentParser
import Foundation
import HedosKernel

struct Image: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "image",
        abstract: "Generate an image from a prompt and write it to a file.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    @Argument(help: "The image prompt.")
    var prompt: String

    @Option(name: .long, help: "Number of denoising steps.")
    var steps: Int?

    @Option(name: .long, help: "Random seed.")
    var seed: Int?

    @Option(name: [.customShort("o"), .long], help: "Output file path (.png).")
    var output: String?

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf, capability: .image)

        var fields: [String: JSONValue] = ["prompt": .string(prompt)]
        if let steps { fields["steps"] = .int(steps) }
        if let seed { fields["seed"] = .int(seed) }

        let jobID = try await kernel.submit(record.id, .image, payload: .object(fields))

        var artifactIDs: [String] = []
        var failure: String?
        loop: for await event in await kernel.scheduler.events(id: jobID) {
            switch event {
            case .status(let status):
                if !global.json { Out.err(status) }
            case .progress(let progress):
                if !global.json, let step = progress.step, let total = progress.totalSteps {
                    Out.err("step \(step)/\(total)")
                }
            case .done(let ids):
                artifactIDs = ids
                break loop
            case .failed(let message):
                failure = message
                break loop
            case .cancelled:
                failure = "generation cancelled."
                break loop
            default:
                break
            }
        }

        if let failure { throw CLIError(failure) }
        guard let first = artifactIDs.first,
            let stored = try await kernel.artifactStore.url(id: first)
        else { throw CLIError("\(record.displayName) produced no image.") }

        let finalPath = try deliver(stored, to: output)
        if global.json {
            try Out.json(MediaReport(model: record.id, path: finalPath))
        } else {
            Out.line(finalPath)
        }
    }
}
