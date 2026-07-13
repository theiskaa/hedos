import HedosKernel
import SwiftUI

enum Fit {
    static func short(_ record: ModelRecord) -> String? {
        guard let assessment = record.fit else { return nil }
        switch assessment.verdict {
        case .runsWell: return "runs well"
        case .tightFit: return "tight fit"
        case .tooLarge: return "too large"
        }
    }

    static func rank(_ record: ModelRecord) -> Int {
        switch record.fit?.verdict {
        case .runsWell: 0
        case .tightFit: 1
        case .tooLarge: 2
        case nil: 3
        }
    }

    static func prefers(_ first: ModelRecord, over second: ModelRecord) -> Bool {
        if rank(first) != rank(second) {
            return rank(first) < rank(second)
        }
        return (first.footprintMB ?? 0) > (second.footprintMB ?? 0)
    }

    static func recommendation(in records: [ModelRecord]) -> ModelRecord? {
        let ready = records.filter {
            $0.state == .ready && Launcher.destination(for: $0) == .chat
        }
        return ready.min(by: prefers)
    }

    static func duplicateInsight(
        _ record: ModelRecord, in summary: DiscoverySummary?
    ) -> DuplicateGroup? {
        guard let summary, let path = record.primaryWeightPath else { return nil }
        return summary.duplicates.first { $0.paths.contains(path) }
    }
}
