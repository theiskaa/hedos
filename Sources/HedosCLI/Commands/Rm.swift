import ArgumentParser
import Foundation
import HedosKernel

struct Rm: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "rm",
        abstract: "Delete an installed model — files to the Trash, entry off the shelf.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    @Flag(
        name: [.long, .customShort("y")],
        help: "Actually delete; without it rm only prints what would go.")
    var yes = false

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf)
        let preview: ModelDeletionPreview
        do {
            preview = try await kernel.deletionPreview(record.id)
        } catch {
            throw CLIError(error.localizedDescription)
        }
        guard yes else {
            try dryRun(preview)
            throw CLIError("nothing deleted — re-run with --yes to confirm.")
        }
        let report: ModelDeletionReport
        do {
            report = try await kernel.deleteModel(record.id)
        } catch {
            throw CLIError(error.localizedDescription)
        }
        if global.json {
            try Out.json(
                RmReport(
                    model: report.modelID, name: report.name, kind: report.kind.rawValue,
                    deleted: true, viaDaemon: report.daemonDeleted,
                    paths: report.trashedPaths, bytes: report.freedBytesEstimate))
        } else if report.daemonDeleted {
            Out.line("deleted \(report.name) via the Ollama daemon")
        } else if report.trashedPaths.isEmpty {
            Out.line("forgot \(report.name) — its files were already gone")
        } else {
            let items = report.trashedPaths.count == 1 ? "1 item" : "\(report.trashedPaths.count) items"
            Out.line(
                "moved \(items) to the Trash (~\(ByteFormat.string(report.freedBytesEstimate))) — deleted \(report.name)"
            )
        }
    }

    private func dryRun(_ preview: ModelDeletionPreview) throws {
        if global.json {
            try Out.json(
                RmReport(
                    model: preview.modelID, name: preview.name, kind: preview.kind.rawValue,
                    deleted: false, viaDaemon: preview.viaDaemon,
                    paths: preview.paths, bytes: preview.bytesEstimate))
            return
        }
        Out.line("\(preview.name) · \(preview.kind.rawValue) · ~\(ByteFormat.string(preview.bytesEstimate))")
        if preview.missing {
            Out.line("  files already gone — would only forget the entry")
        } else if preview.viaDaemon {
            Out.line("  asks the Ollama daemon to delete \(preview.name); shared layers stay")
        } else {
            for path in preview.paths {
                Out.line("  \(path)")
            }
        }
    }
}

struct RmReport: Encodable {
    let model: String
    let name: String
    let kind: String
    let deleted: Bool
    let viaDaemon: Bool
    let paths: [String]
    let bytes: Int64
}
