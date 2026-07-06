import Foundation

public enum FitVerdict: String, Codable, Hashable, Sendable {
    case runsWell
    case tightFit
    case tooLarge

    public static let memoryOverheadFactor = 1.25
    public static let runsWellFraction = 0.75
    public static let tightFitFraction = 0.95

    public struct Assessment: Hashable, Sendable {
        public let verdict: FitVerdict
        public let requiredBytes: Int64

        public init(verdict: FitVerdict, requiredBytes: Int64) {
            self.verdict = verdict
            self.requiredBytes = requiredBytes
        }
    }

    public static func assess(footprintMB: Int?, totalMemoryBytes: UInt64) -> Assessment? {
        guard let footprintMB, footprintMB > 0, totalMemoryBytes > 0 else { return nil }
        let requiredBytes = Int64(Double(footprintMB) * Double(1 << 20) * memoryOverheadFactor)
        let share = Double(requiredBytes) / Double(totalMemoryBytes)
        let verdict: FitVerdict =
            if share < runsWellFraction {
                .runsWell
            } else if share < tightFitFraction {
                .tightFit
            } else {
                .tooLarge
            }
        return Assessment(verdict: verdict, requiredBytes: requiredBytes)
    }
}

extension ModelRecord {
    public var fit: FitVerdict.Assessment? {
        FitVerdict.assess(
            footprintMB: footprintMB,
            totalMemoryBytes: ProcessInfo.processInfo.physicalMemory)
    }
}
