import CryptoKit
import Foundation

struct UserRuntimeStore: Sendable {
    let directory: URL

    init(directory: URL) {
        self.directory = directory
    }

    func load(reservedIDs: Set<String>) -> (manifests: [RuntimeManifest], issues: [String]) {
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
                manifest.contentHash = Self.consentHash(
                    manifestText: text, directory: manifestDirectory, manifest: manifest)
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

    static func contentHash(for text: String) -> String {
        SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    static func consentHash(
        manifestText: String, directory: URL?, manifest: RuntimeManifest
    ) -> String {
        guard let directory else { return contentHash(for: manifestText) }

        var hasher = SHA256()
        func absorb(path: String, content: Data?) {
            hasher.update(data: Data(path.utf8))
            hasher.update(data: Data([0]))
            let count = content.map { UInt64($0.count) } ?? UInt64.max
            withUnsafeBytes(of: count.littleEndian) { hasher.update(data: Data($0)) }
            if let content { hasher.update(data: content) }
        }

        absorb(path: "manifest.toml", content: Data(manifestText.utf8))
        if let serve = manifest.serve {
            absorb(
                path: serve.entrypoint,
                content: try? Data(contentsOf: directory.appendingPathComponent(serve.entrypoint)))
        }
        if let env = manifest.env {
            absorb(
                path: env.lockfile,
                content: try? Data(contentsOf: directory.appendingPathComponent(env.lockfile)))
        }
        let siblings =
            ((try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: [.isRegularFileKey],
                options: [.skipsHiddenFiles])) ?? [])
            .filter { $0.pathExtension.lowercased() == "py" }
            .filter {
                ((try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) ?? false)
            }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
        for sibling in siblings {
            absorb(path: sibling.lastPathComponent, content: try? Data(contentsOf: sibling))
        }

        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}
