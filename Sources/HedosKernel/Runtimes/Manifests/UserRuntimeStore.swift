import Foundation

public struct UserRuntimeStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    public func load(reservedIDs: Set<String>) -> (manifests: [RuntimeManifest], issues: [String]) {
        let fm = FileManager.default
        guard
            let entries = try? fm.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: [.isDirectoryKey],
                options: [.skipsHiddenFiles])
        else { return ([], []) }

        var manifests: [RuntimeManifest] = []
        var issues: [String] = []
        var seen = Set<String>()

        for entry in entries.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
            let isDirectory =
                (try? entry.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            let manifestURL: URL
            let manifestDirectory: URL?
            let label: String
            if isDirectory {
                manifestURL = entry.appendingPathComponent("manifest.toml")
                manifestDirectory = entry
                label = "runtimes.d/\(entry.lastPathComponent)/manifest.toml"
                guard fm.fileExists(atPath: manifestURL.path) else { continue }
            } else {
                guard entry.pathExtension.lowercased() == "toml" else { continue }
                manifestURL = entry
                manifestDirectory = nil
                label = "runtimes.d/\(entry.lastPathComponent)"
            }

            do {
                let text = try String(contentsOf: manifestURL, encoding: .utf8)
                let table = try TOMLLite.parse(text)
                var manifest = try RuntimeManifest.load(table: table, directory: manifestDirectory)
                if let manifestDirectory {
                    manifest.provenance = RuntimeProvenance.read(in: manifestDirectory)
                }
                if manifest.provenance?.isCommunity == true, manifest.vm == nil {
                    issues.append(
                        "\(label): community runtimes run contained — \"\(manifest.id)\" has no [vm] section"
                    )
                    continue
                }
                if reservedIDs.contains(manifest.id) {
                    issues.append("\(label): id \"\(manifest.id)\" is reserved")
                    continue
                }
                if seen.contains(manifest.id) {
                    issues.append("\(label): duplicate id \"\(manifest.id)\" — keeping the first")
                    continue
                }
                if manifestDirectory == nil && (manifest.serve != nil || manifest.env != nil) {
                    issues.append(
                        "\(label): [serve] and [env] manifests must live in a directory beside their files"
                    )
                    continue
                }
                if manifest.detect == nil {
                    issues.append(
                        "\(label): manifest \"\(manifest.id)\" has no detect rule and will never match a model"
                    )
                }
                seen.insert(manifest.id)
                manifests.append(manifest)
            } catch let error as TOMLParseError {
                issues.append("\(label): \(error.description)")
            } catch let error as ManifestValidationError {
                issues.append("\(label): \(error.message)")
            } catch {
                issues.append("\(label): \(error.localizedDescription)")
            }
        }
        return (manifests, issues)
    }
}
