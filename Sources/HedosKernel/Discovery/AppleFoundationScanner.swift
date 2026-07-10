import Foundation

struct AppleFoundationScanner: StoreScanner {
    var kinds: Set<SourceKind> { [.builtin] }

    static let sourcePath = "/System/Library/Frameworks/FoundationModels.framework"

    private let backend: any AppleFoundationBackend

    init(backend: any AppleFoundationBackend = SystemFoundationBackend()) {
        self.backend = backend
    }

    func scan() async -> ScanResult {
        switch backend.availability() {
        case .available:
            return ScanResult(discovered: [
                DiscoveredModel(
                    name: "Apple Intelligence",
                    source: ModelSource(kind: .builtin, path: Self.sourcePath),
                    modalityHint: .text,
                    capabilitiesHint: [.chat, .complete],
                    executionHint: .stream,
                    footprintBytes: 0)
            ])
        case .notEnabled:
            return ScanResult(issues: [
                "Apple Intelligence is turned off — turn it on in System Settings to put Apple's model on the shelf."
            ])
        case .notReady:
            return ScanResult(issues: [
                "Apple's model is still downloading — it will appear on the shelf when it's ready."
            ])
        case .notEligible:
            return ScanResult()
        }
    }
}
