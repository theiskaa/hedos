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
        let base = directory.standardizedFileURL.path
        var files: [(String, URL)] = []
        if let enumerator = FileManager.default.enumerator(
            at: directory, includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles])
        {
            for case let url as URL in enumerator {
                guard
                    (try? url.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true
                else { continue }
                let relative = String(url.standardizedFileURL.path.dropFirst(base.count + 1))
                if relative == "manifest.toml" { continue }
                files.append((relative, url))
            }
        }
        for (relative, url) in files.sorted(by: { $0.0 < $1.0 }) {
            absorb(path: relative, content: try? Data(contentsOf: url))
        }

        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}
