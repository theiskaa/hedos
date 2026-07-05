import Foundation

/// LM Studio keeps `<root>/<org>/<repo>/<file>.gguf`. Two historical root
/// locations exist; both are checked.
public struct LMStudioScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.lmStudio] }
    public let roots: [URL]

    public init(roots: [URL]) {
        self.roots = roots
    }

    public static func defaultRoots() -> [URL] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [
            home.appendingPathComponent(".lmstudio/models"),
            home.appendingPathComponent(".cache/lm-studio/models"),
        ]
    }

    public func scan() async -> ScanResult {
        scanSynchronously()
    }

    // NSEnumerator iteration is unavailable in async contexts.
    private func scanSynchronously() -> ScanResult {
        let fm = FileManager.default
        var result = ScanResult()

        for root in roots where fm.fileExists(atPath: root.path) {
            // producesRelativePathURLs sidesteps /var vs /private/var prefix
            // mismatches that break string-based relative-path math.
            guard
                let enumerator = fm.enumerator(
                    at: root, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey],
                    options: [.skipsHiddenFiles, .producesRelativePathURLs])
            else { continue }
            for case let url as URL in enumerator {
                guard url.pathExtension.lowercased() == "gguf",
                    (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
                else { continue }
                let size = Int64((try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0)
                let rel = url.relativePath.split(separator: "/")
                let repo = rel.count >= 3 ? rel.dropLast().joined(separator: "/") : nil
                let hint = ModalityHints.gguf
                result.discovered.append(
                    DiscoveredModel(
                        name: url.deletingPathExtension().lastPathComponent,
                        source: ModelSource(kind: .lmStudio, path: url.path, repo: repo),
                        modalityHint: hint.modality,
                        capabilitiesHint: hint.capabilities,
                        executionHint: hint.execution,
                        footprintBytes: size,
                        primaryWeightPath: url.path))
            }
        }
        return result
    }
}
