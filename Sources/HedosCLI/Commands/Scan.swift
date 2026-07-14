import ArgumentParser
import HedosKernel

struct Scan: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "scan",
        abstract: "Discover models on this machine and refresh the shelf.")

    @OptionGroup var global: GlobalOptions

    func run() async throws {
        let kernel = Session.kernel()
        let summary = try await kernel.discover()
        if global.json {
            try Out.json(ScanReport(summary))
        } else {
            Out.line(summary.headline)
            for issue in summary.issues { Out.err("issue: \(issue)") }
        }
    }
}

struct ScanReport: Encodable {
    struct KindStat: Encodable {
        let count: Int
        let bytes: Int64
    }
    let totalCount: Int
    let totalBytes: Int64
    let headline: String
    let perKind: [String: KindStat]
    let issues: [String]

    init(_ summary: DiscoverySummary) {
        totalCount = summary.totalCount
        totalBytes = summary.totalBytes
        headline = summary.headline
        issues = summary.issues
        var kinds: [String: KindStat] = [:]
        for (kind, stat) in summary.perKind {
            kinds[kind.rawValue] = KindStat(count: stat.count, bytes: stat.bytes)
        }
        perKind = kinds
    }
}
