import Foundation

/// Sweeps explicit, listed folders for loose models — never a disk crawl.
/// Depth is capped at 2. Finds standalone `.gguf` files (kind `.file`) and
/// folders shaped like a model bundle: `config.json` + at least one
/// safetensors (kind `.folder`).
public struct LooseFileScanner: StoreScanner {
    public var kinds: Set<SourceKind> { [.file, .folder] }
    public let directories: [URL]
    private let maxDepth = 2

    public init(directories: [URL]) {
        self.directories = directories
    }

    public static func defaultDirectories() -> [URL] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return [
            home.appendingPathComponent("Downloads"),
            home.appendingPathComponent("Models"),
        ]
    }

    public func scan() async -> ScanResult {
        var result = ScanResult()
        for dir in directories {
            sweep(dir, depth: 0, into: &result)
        }
        return result
    }

    private func sweep(_ dir: URL, depth: Int, into result: inout ScanResult) {
        guard depth <= maxDepth else { return }
        let fm = FileManager.default
        guard
            let entries = try? fm.contentsOfDirectory(
                at: dir, includingPropertiesForKeys: [.isDirectoryKey, .fileSizeKey],
                options: [.skipsHiddenFiles])
        else { return }

        for entry in entries {
            let values = try? entry.resourceValues(forKeys: [.isDirectoryKey, .fileSizeKey])
            if values?.isDirectory == true {
                if let bundle = folderBundle(at: entry) {
                    result.discovered.append(bundle)
                } else {
                    sweep(entry, depth: depth + 1, into: &result)
                }
            } else if entry.pathExtension.lowercased() == "gguf" {
                let hint = ModalityHints.gguf
                result.discovered.append(
                    DiscoveredModel(
                        name: entry.deletingPathExtension().lastPathComponent,
                        source: ModelSource(kind: .file, path: entry.path),
                        modalityHint: hint.modality,
                        capabilitiesHint: hint.capabilities,
                        executionHint: hint.execution,
                        footprintBytes: Int64(values?.fileSize ?? 0),
                        primaryWeightPath: entry.path))
            }
        }
    }

    private func folderBundle(at dir: URL) -> DiscoveredModel? {
        let fm = FileManager.default
        guard
            let entries = try? fm.contentsOfDirectory(
                at: dir, includingPropertiesForKeys: [.fileSizeKey],
                options: [.skipsHiddenFiles])
        else { return nil }
        let names = Set(entries.map(\.lastPathComponent))
        guard names.contains("config.json"),
            entries.contains(where: { $0.pathExtension == "safetensors" })
        else { return nil }

        var hint = ModalityHints.fromConfigJSON(at: dir.appendingPathComponent("config.json"))
            ?? ModalityHints.Hint(modality: nil, capabilities: [], execution: .sync)
        if names.contains("model_index.json") {
            hint = ModalityHints.fromModelIndex(at: dir.appendingPathComponent("model_index.json"))
        }

        var total: Int64 = 0
        var largest: (path: String, size: Int64)?
        for entry in entries {
            let size = Int64((try? entry.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0)
            total += size
            if entry.pathExtension == "safetensors", size > (largest?.size ?? -1) {
                largest = (entry.path, size)
            }
        }

        return DiscoveredModel(
            name: dir.lastPathComponent,
            source: ModelSource(kind: .folder, path: dir.path),
            modalityHint: hint.modality,
            capabilitiesHint: hint.capabilities,
            executionHint: hint.execution,
            footprintBytes: total,
            primaryWeightPath: largest?.path)
    }
}
