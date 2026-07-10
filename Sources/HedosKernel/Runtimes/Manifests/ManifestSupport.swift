import Foundation

public protocol ManifestBacked {
    var manifest: RuntimeManifest { get }
}

extension RuntimeManifest {
    var alternativeIDs: [RuntimeID] {
        alternatives.map(RuntimeID.init(rawValue:))
    }
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
    static func errorSummary(_ raw: String) -> String {
        let lines = raw.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        guard let last = lines.last else { return "the runtime stopped without output" }
        return String(last.suffix(300))
    }

    static func slug(_ id: String) -> String {
        id.map { $0.isLetter || $0.isNumber ? $0 : "-" }.reduce(into: "") { $0.append($1) }
    }

    static func profileURL(network: Bool) -> URL? {
        RuntimeBundle.directory(named: "generic")?
            .appendingPathComponent(network ? "generic-net-on.sb" : "generic-net-off.sb")
    }

    static func defaultWorkdirRoot() -> URL {
        SidecarWorkdir.defaultRoot()
    }

    static func workdir(for manifest: RuntimeManifest, root: URL) throws -> URL {
        try SidecarWorkdir.directory(root: root, name: slug(manifest.id))
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

    static func substitutedForVM(command: String, payload: JSONValue) throws -> [String] {
        let prompt = promptText(from: payload)
        var tokens: [String] = []
        for token in command.split(separator: " ").map(String.init) {
            var expanded = token
            expanded = expanded.replacingOccurrences(of: "{model}", with: VMGuestPath.model)
            expanded = expanded.replacingOccurrences(of: "{prompt}", with: prompt)
            expanded = expanded.replacingOccurrences(of: "{workdir}", with: VMGuestPath.workdir)
            expanded = expanded.replacingOccurrences(of: "{outputs}", with: VMGuestPath.outputs)
            expanded = expanded.replacingOccurrences(
                of: "{resources}", with: VMGuestPath.resources)
            expanded = expanded.replacingOccurrences(of: "{python}", with: "python3")
            tokens.append(expanded)
        }
        guard !tokens.isEmpty else {
            throw KernelError.runtimeFailed("the manifest command is empty")
        }
        return tokens
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
        manifest: RuntimeManifest, environments: EnvironmentManager = .shared,
        progress: @escaping @Sendable (String) -> Void
    ) async throws -> URL? {
        guard let env = manifest.env else { return nil }
        guard let directory = manifest.directory else {
            throw KernelError.runtimeFailed(
                "manifest \(manifest.id) declares [env] but has no directory")
        }
        return try await environments.prepare(
            runtimeID: manifest.id,
            lockfile: directory.appendingPathComponent(env.lockfile),
            progress: progress)
    }
}
