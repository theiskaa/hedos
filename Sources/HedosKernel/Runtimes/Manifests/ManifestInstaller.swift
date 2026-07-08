import Foundation

public struct RuntimeInstallPreview: Sendable, Hashable {
    public var id: String
    public var capabilities: [String]
    public var execution: String
    public var image: String
    public var setup: [String]
    public var paths: [String]
    public var detectSummary: String?
    public var vmAssetDownloadMB: Int?
    public var sourceURL: URL
}

struct ManifestInstaller {
    let runtimesDirectory: URL
    let reservedIDs: Set<String>

    private func loadManifest(from source: URL) throws -> (RuntimeManifest, URL) {
        let manifestURL: URL
        let isDirectory =
            (try? source.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
        if isDirectory {
            manifestURL = source.appendingPathComponent("manifest.toml")
        } else {
            manifestURL = source
        }
        guard FileManager.default.fileExists(atPath: manifestURL.path) else {
            throw ManifestValidationError(message: "no manifest.toml at \(source.path)")
        }
        let text = try String(contentsOf: manifestURL, encoding: .utf8)
        let table = try TOMLLite.parse(text)
        let manifest = try RuntimeManifest.load(
            table: table, directory: isDirectory ? source : nil)
        return (manifest, manifestURL)
    }

    func preview(from source: URL, vmAssetState: VMAssetState) throws -> RuntimeInstallPreview {
        let (manifest, _) = try loadManifest(from: source)
        guard let vm = manifest.vm else {
            throw ManifestValidationError(
                message:
                    "community runtimes run contained — \(manifest.id) needs a [vm] section")
        }
        guard !reservedIDs.contains(manifest.id) else {
            throw ManifestValidationError(message: "id \"\(manifest.id)\" is reserved")
        }
        var downloadMB: Int?
        if case .absent(let approx) = vmAssetState {
            downloadMB = approx
        }
        var detectSummary: String?
        if let detect = manifest.detect {
            if let file = detect.file {
                detectSummary =
                    detect.contains.map { "models whose \(file) mentions \($0)" }
                    ?? "models carrying \(file)"
            } else if let ext = detect.fileExtension {
                detectSummary = ".\(ext) files"
            }
        }
        return RuntimeInstallPreview(
            id: manifest.id,
            capabilities: manifest.capabilities.map(\.rawValue),
            execution: manifest.execution.rawValue,
            image: vm.image,
            setup: vm.setup,
            paths: manifest.permissions.paths,
            detectSummary: detectSummary,
            vmAssetDownloadMB: downloadMB,
            sourceURL: source)
    }

    func install(from source: URL) throws -> String {
        let (manifest, manifestURL) = try loadManifest(from: source)
        guard manifest.vm != nil else {
            throw ManifestValidationError(
                message:
                    "community runtimes run contained — \(manifest.id) needs a [vm] section")
        }
        guard !reservedIDs.contains(manifest.id) else {
            throw ManifestValidationError(message: "id \"\(manifest.id)\" is reserved")
        }
        let fm = FileManager.default
        let destination = runtimesDirectory.appendingPathComponent(
            ManifestSupport.slug(manifest.id), isDirectory: true)
        guard !fm.fileExists(atPath: destination.path) else {
            throw ManifestValidationError(
                message: "a runtime named \(manifest.id) is already installed")
        }
        try fm.createDirectory(at: runtimesDirectory, withIntermediateDirectories: true)
        if manifest.directory != nil {
            try fm.copyItem(at: source, to: destination)
        } else {
            try fm.createDirectory(at: destination, withIntermediateDirectories: true)
            try fm.copyItem(
                at: manifestURL, to: destination.appendingPathComponent("manifest.toml"))
        }
        try RuntimeProvenance(origin: RuntimeProvenance.communityOrigin)
            .write(in: destination)
        return manifest.id
    }

    func uninstall(id: String) throws {
        let destination = runtimesDirectory.appendingPathComponent(
            ManifestSupport.slug(id), isDirectory: true)
        guard FileManager.default.fileExists(atPath: destination.path) else {
            throw ManifestValidationError(message: "no installed runtime named \(id)")
        }
        guard RuntimeProvenance.read(in: destination)?.isCommunity == true else {
            throw ManifestValidationError(
                message: "\(id) was not installed by Hedos — remove it by hand from runtimes.d")
        }
        try FileManager.default.removeItem(at: destination)
    }
}
