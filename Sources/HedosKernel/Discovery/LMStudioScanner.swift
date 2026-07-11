import Foundation

public struct LMStudioScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.lmStudio] }
    public let roots: [URL]

    public init(roots: [URL]) {
        self.roots = roots
    }

    public static func defaultRoots(
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> [URL] {
        return [
            home.appendingPathComponent(".lmstudio/models"),
            home.appendingPathComponent(".cache/lm-studio/models"),
        ]
    }

    public func scan() async -> ScanResult {
        scanSynchronously()
    }

    private func scanSynchronously() -> ScanResult {
        let fm = FileManager.default
        var result = ScanResult()

        for root in roots where fm.fileExists(atPath: root.path) {
            guard fm.isReadableFile(atPath: root.path),
                let enumerator = fm.enumerator(
                    at: root, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey],
                    options: [.skipsHiddenFiles, .producesRelativePathURLs])
            else {
                result.failedKinds.insert(.lmStudio)
                continue
            }
            var ggufFiles: [(url: URL, bytes: Int64)] = []
            for case let url as URL in enumerator {
                guard url.pathExtension.lowercased() == "gguf",
                    !Identification.isMmprojName(url.lastPathComponent),
                    (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true
                else { continue }
                let size = Int64((try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0)
                ggufFiles.append((url, size))
            }
            let (models, issues) = GGUFShards.discoveredModels(
                from: ggufFiles, kind: .lmStudio, repo: Self.repoFromRelativePath)
            result.discovered.append(contentsOf: models)
            result.issues.append(contentsOf: issues)
        }
        return result
    }

    private static func repoFromRelativePath(_ url: URL) -> String? {
        let rel = url.relativePath.split(separator: "/")
        return rel.count >= 3 ? rel.dropLast().joined(separator: "/") : nil
    }
}
