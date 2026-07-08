import Foundation

public protocol ManifestBacked {
    var manifest: RuntimeManifest { get }
}

public struct ManifestConsentInfo: Sendable, Hashable {
    public let id: String
    public let paths: [String]

    public init(id: String, paths: [String]) {
        self.id = id
        self.paths = paths
    }
}

enum ManifestSupport {
    static func slug(_ id: String) -> String {
        id.map { $0.isLetter || $0.isNumber ? $0 : "-" }.reduce(into: "") { $0.append($1) }
    }

    static func profileURL(network: Bool) -> URL? {
        RuntimeBundle.directory(named: "generic")?
            .appendingPathComponent(network ? "generic-net-on.sb" : "generic-net-off.sb")
    }

    static func workdir(for manifest: RuntimeManifest) throws -> URL {
        let workdir = Registry.defaultDirectory()
            .appendingPathComponent("workdirs/\(slug(manifest.id))", isDirectory: true)
        try FileManager.default.createDirectory(at: workdir, withIntermediateDirectories: true)
        return workdir
    }

    static func canonicalPath(_ url: URL) -> String {
        var buffer = [CChar](repeating: 0, count: Int(PATH_MAX))
        guard realpath(url.path, &buffer) != nil else {
            return url.resolvingSymlinksInPath().path
        }
        return String(cString: buffer)
    }

    static func sandboxArguments(
        profile: URL, envDir: URL?, manifest: RuntimeManifest, record: ModelRecord, workdir: URL
    ) -> [String] {
        let fm = FileManager.default
        let paths = SidecarModelPaths.resolve(record)
        let anchor = manifest.directory ?? workdir
        let venv = envDir.map { canonicalPath($0) } ?? canonicalPath(anchor)
        let uvPythonRoot: String
        if let envDir {
            let realPython = URL(
                fileURLWithPath: canonicalPath(envDir.appendingPathComponent("bin/python")))
            uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent().path
        } else {
            uvPythonRoot = canonicalPath(anchor)
        }
        let tmp = URL(fileURLWithPath: canonicalPath(fm.temporaryDirectory))
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")
        return [
            "-f", profile.path,
            "-D", "VENV=\(venv)",
            "-D", "UVPY=\(uvPythonRoot)",
            "-D", "MODEL=\(canonicalPath(URL(fileURLWithPath: paths.sandboxRoot)))",
            "-D", "WORKDIR=\(canonicalPath(workdir))",
            "-D", "RESOURCES=\(canonicalPath(anchor))",
            "-D", "TMP=\(tmp.path)",
            "-D", "CACHE=\(darwinCache.path)",
        ]
    }

    static func promptText(from payload: JSONValue) -> String {
        guard case .object(let object) = payload else { return "" }
        if case .string(let prompt)? = object["prompt"] {
            return prompt
        }
        guard case .array(let entries)? = object["messages"] else { return "" }
        for entry in entries.reversed() {
            guard case .object(let fields) = entry,
                case .string(let role)? = fields["role"], role == "user",
                case .string(let content)? = fields["content"]
            else { continue }
            return content
        }
        return ""
    }

    static func substituted(
        command: String, record: ModelRecord, payload: JSONValue,
        workdir: URL, outputs: URL, envDir: URL?
    ) throws -> [String] {
        let paths = SidecarModelPaths.resolve(record)
        let prompt = promptText(from: payload)
        var tokens: [String] = []
        for token in command.split(separator: " ").map(String.init) {
            var expanded = token
            expanded = expanded.replacingOccurrences(of: "{model}", with: paths.snapshot)
            expanded = expanded.replacingOccurrences(of: "{prompt}", with: prompt)
            expanded = expanded.replacingOccurrences(of: "{workdir}", with: workdir.path)
            expanded = expanded.replacingOccurrences(of: "{outputs}", with: outputs.path)
            if expanded.contains("{python}") {
                guard let envDir else {
                    throw KernelError.runtimeFailed(
                        "the command uses {python} but the manifest declares no [env]")
                }
                expanded = expanded.replacingOccurrences(
                    of: "{python}", with: envDir.appendingPathComponent("bin/python").path)
            }
            tokens.append(expanded)
        }
        guard !tokens.isEmpty else {
            throw KernelError.runtimeFailed("the manifest command is empty")
        }
        return tokens
    }

    static func prepareEnvironmentIfNeeded(
        manifest: RuntimeManifest, progress: @escaping @Sendable (String) -> Void
    ) async throws -> URL? {
        guard let env = manifest.env else { return nil }
        guard let directory = manifest.directory else {
            throw KernelError.runtimeFailed(
                "manifest \(manifest.id) declares [env] but has no directory")
        }
        return try await EnvironmentManager.shared.prepare(
            runtimeID: manifest.id,
            lockfile: directory.appendingPathComponent(env.lockfile),
            progress: progress)
    }
}
