import ArgumentParser
import HedosKernel

struct Ls: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "ls",
        abstract: "List models with fit verdict, tier, warm state, and store.")

    @OptionGroup var global: GlobalOptions

    @Flag(name: .long, help: "Rescan the machine before listing.")
    var scan = false

    @Option(name: .long, help: "Only models serving this capability (chat, image, speak, embed, ...).")
    var capability: String?

    func run() async throws {
        let kernel = Session.kernel()
        if scan { _ = try await kernel.discover() }
        var shelf = try await Session.shelf(kernel)
        if let capability {
            guard Capabilities.known.contains(capability) else {
                throw CLIError(
                    "unknown capability \"\(capability)\" — valid: "
                    + Capabilities.known.sorted().joined(separator: ", "))
            }
            let cap = Capability(rawValue: capability)
            shelf = shelf.filter { $0.capabilities.contains(cap) }
        }
        let residents = await kernel.residentModels()
        func isWarm(_ record: ModelRecord) -> Bool {
            residents.contains {
                $0.modelID == record.id || ($0.origin == .ollama && $0.name == record.name)
            }
        }
        let rows = shelf
            .sorted { $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending }
            .map { ModelRow($0, warm: isWarm($0)) }

        if global.json {
            try Out.json(rows)
        } else if rows.isEmpty {
            Out.line("shelf is empty — run `hedos scan`, or `hedos pull <model>` to fetch one.")
        } else {
            Out.line(ModelRow.table(rows))
        }
    }
}

struct ModelRow: Encodable {
    let id: String
    let name: String
    let store: String
    let runtime: String
    let tier: String
    let capabilities: [String]
    let fit: String?
    let warm: Bool
    let footprintMB: Int?

    init(_ record: ModelRecord, warm: Bool) {
        id = record.id
        name = record.displayName
        store = record.source.kind.rawValue
        runtime = record.runtime.id?.rawValue ?? "unresolved"
        tier = record.runtime.tier.rawValue
        capabilities = record.capabilities.map(\.rawValue)
        fit = record.fit?.verdict.label
        self.warm = warm
        footprintMB = record.footprintMB
    }

    static func table(_ rows: [ModelRow]) -> String {
        let header = ["", "NAME", "FIT", "TIER", "RUNTIME", "STORE", "CAPABILITIES"]
        var cells: [[String]] = [header]
        for row in rows {
            cells.append([
                row.warm ? "●" : "○",
                row.name,
                row.fit ?? "—",
                row.tier,
                row.runtime,
                row.store,
                row.capabilities.joined(separator: ","),
            ])
        }
        let columns = header.count
        var widths = [Int](repeating: 0, count: columns)
        for line in cells {
            for index in 0..<columns { widths[index] = max(widths[index], line[index].count) }
        }
        return cells.map { line in
            let joined = line.enumerated()
                .map { index, value in
                    index == columns - 1
                        ? value
                        : value + String(repeating: " ", count: max(0, widths[index] - value.count))
                }
                .joined(separator: "  ")
            var trimmed = Substring(joined)
            while trimmed.last == " " { trimmed = trimmed.dropLast() }
            return String(trimmed)
        }.joined(separator: "\n")
    }
}

extension FitVerdict {
    var label: String {
        switch self {
        case .runsWell: return "runs well"
        case .tightFit: return "tight"
        case .tooLarge: return "too large"
        }
    }
}
