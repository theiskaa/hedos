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
    public let network: Bool

    public init(id: String, paths: [String], network: Bool = false) {
        self.id = id
        self.paths = paths
        self.network = network
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
        guard let real = realpath(url.path, &buffer) else {
            return url.resolvingSymlinksInPath().path
        }
        return String(cString: real)
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
        return conversationText(from: payload)
    }

    static func conversationText(from payload: JSONValue) -> String {
        guard case .object(let object) = payload,
            case .array(let entries)? = object["messages"]
        else { return "" }
        var lines: [String] = []
        for entry in entries {
            guard case .object(let fields) = entry,
                case .string(let role)? = fields["role"],
                case .string(let content)? = fields["content"]
            else { continue }
            lines.append("\(role): \(content)")
        }
        return lines.joined(separator: "\n")
    }

    static let maxOutputFileBytes = 256 * 1024 * 1024

    static func boundedOutputData(at url: URL, limit: Int = maxOutputFileBytes) throws -> Data {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        guard size <= limit else {
            throw KernelError.runtimeFailed(
                "output file \(url.lastPathComponent) is \(size) bytes, larger than the \(limit) cap")
        }
        return try Data(contentsOf: url)
    }

    static func expandPlaceholders(_ token: String, _ replacements: [String: String]) -> String {
        let keys = replacements.keys.sorted { $0.count > $1.count }
        var result = ""
        var index = token.startIndex
        while index < token.endIndex {
            var matched = false
            for key in keys where token[index...].hasPrefix(key) {
                result += replacements[key] ?? ""
                index = token.index(index, offsetBy: key.count)
                matched = true
                break
            }
            if !matched {
                result.append(token[index])
                index = token.index(after: index)
            }
        }
        return result
    }

    static func substitutedForVM(command: String, payload: JSONValue) throws -> [String] {
        let replacements: [String: String] = [
            "{model}": VMGuestPath.model,
            "{prompt}": promptText(from: payload),
            "{workdir}": VMGuestPath.workdir,
            "{outputs}": VMGuestPath.outputs,
            "{resources}": VMGuestPath.resources,
            "{python}": "python3",
        ]
        let tokens = command.split(separator: " ").map {
            expandPlaceholders(String($0), replacements)
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
        if command.contains("{python}"), envDir == nil {
            throw KernelError.runtimeFailed(
                "the command uses {python} but the manifest declares no [env]")
        }
        var replacements: [String: String] = [
            "{model}": paths.snapshot,
            "{prompt}": promptText(from: payload),
            "{workdir}": workdir.path,
            "{outputs}": outputs.path,
        ]
        if let envDir {
            replacements["{python}"] = envDir.appendingPathComponent("bin/python").path
        }
        let tokens = command.split(separator: " ").map {
            expandPlaceholders(String($0), replacements)
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
