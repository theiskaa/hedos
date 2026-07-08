import Foundation

public struct ManifestValidationError: Error, Sendable, CustomStringConvertible {
    public let message: String

    public var description: String { message }
}

public struct ManifestDetect: Sendable, Hashable {
    public var file: String?
    public var contains: String?
    public var fileExtension: String?

    public func matches(_ record: ModelRecord) -> Bool {
        if let fileExtension {
            let candidate = record.primaryWeightPath ?? record.source.path
            return candidate.lowercased().hasSuffix(".\(fileExtension.lowercased())")
        }
        guard let file else { return false }
        let paths = SidecarModelPaths.resolve(record)
        let target = URL(fileURLWithPath: paths.snapshot).appendingPathComponent(file)
        guard FileManager.default.fileExists(atPath: target.path) else { return false }
        guard let contains else { return true }
        guard let content = try? String(contentsOf: target, encoding: .utf8) else {
            return false
        }
        return content.contains(contains)
    }
}

public struct ManifestEnv: Sendable, Hashable {
    public var manager: String
    public var python: String
    public var lockfile: String
}

public struct ManifestServe: Sendable, Hashable {
    public var entrypoint: String
    public var wireProtocol: String
}

public struct ManifestInvoke: Sendable, Hashable {
    public var command: String
}

public struct ManifestPermissions: Sendable, Hashable {
    public var network: Bool
    public var paths: [String]
}

public struct RuntimeManifest: Sendable, Hashable {
    public var id: String
    public var modalities: [Modality]
    public var capabilities: [Capability]
    public var execution: ExecutionMode
    public var alternatives: [String]
    public var detect: ManifestDetect?
    public var env: ManifestEnv?
    public var serve: ManifestServe?
    public var invoke: ManifestInvoke?
    public var permissions: ManifestPermissions
    public var directory: URL?

    public static func load(table: TOMLTable, directory: URL?) throws -> RuntimeManifest {
        guard let id = table["id"]?.stringValue, !id.isEmpty else {
            throw ManifestValidationError(message: "manifest is missing an id")
        }
        let modalities = (table["modalities"]?.stringArray ?? []).map(Modality.init(rawValue:))
        let capabilities = (table["capabilities"]?.stringArray ?? [])
            .map(Capability.init(rawValue:))
        guard !capabilities.isEmpty else {
            throw ManifestValidationError(message: "manifest \(id) declares no capabilities")
        }
        guard let executionRaw = table["execution"]?.stringValue,
            let execution = ExecutionMode(rawValue: executionRaw)
        else {
            throw ManifestValidationError(
                message: "manifest \(id) has an unknown execution mode")
        }

        var detect: ManifestDetect?
        if let detectTable = table["detect"]?.tableValue {
            let parsed = ManifestDetect(
                file: detectTable["file"]?.stringValue,
                contains: detectTable["contains"]?.stringValue,
                fileExtension: detectTable["extension"]?.stringValue)
            guard parsed.file != nil || parsed.fileExtension != nil else {
                throw ManifestValidationError(
                    message: "manifest \(id) has a detect rule with no file or extension")
            }
            detect = parsed
        }

        var env: ManifestEnv?
        if let envTable = table["env"]?.tableValue {
            guard let lockfile = envTable["lockfile"]?.stringValue else {
                throw ManifestValidationError(
                    message: "manifest \(id) declares [env] without a lockfile")
            }
            env = ManifestEnv(
                manager: envTable["manager"]?.stringValue ?? "uv",
                python: envTable["python"]?.stringValue ?? "3.12",
                lockfile: lockfile)
        }

        var serve: ManifestServe?
        if let serveTable = table["serve"]?.tableValue {
            guard let entrypoint = serveTable["entrypoint"]?.stringValue else {
                throw ManifestValidationError(
                    message: "manifest \(id) declares [serve] without an entrypoint")
            }
            serve = ManifestServe(
                entrypoint: entrypoint,
                wireProtocol: serveTable["protocol"]?.stringValue ?? "ndjson+frames")
        }

        var invoke: ManifestInvoke?
        if let invokeTable = table["invoke"]?.tableValue {
            guard let command = invokeTable["command"]?.stringValue, !command.isEmpty else {
                throw ManifestValidationError(
                    message: "manifest \(id) declares [invoke] without a command")
            }
            invoke = ManifestInvoke(command: command)
        }

        if serve != nil && invoke != nil {
            throw ManifestValidationError(
                message: "manifest \(id) declares both [serve] and [invoke]")
        }
        if serve == nil && invoke == nil {
            throw ManifestValidationError(
                message: "manifest \(id) declares neither [serve] nor [invoke]")
        }

        let jobCapabilities: Set<Capability> = [.image]
        let declaresJob = execution == .job
        let servesJob = !Set(capabilities).isDisjoint(with: jobCapabilities)
        if declaresJob != servesJob {
            throw ManifestValidationError(
                message:
                    "manifest \(id) execution \"\(executionRaw)\" does not match its capabilities")
        }

        let permissionsTable = table["permissions"]?.tableValue ?? [:]
        let permissions = ManifestPermissions(
            network: permissionsTable["network"]?.boolValue ?? false,
            paths: permissionsTable["paths"]?.stringArray ?? ["{model}", "{workdir}"])

        return RuntimeManifest(
            id: id,
            modalities: modalities,
            capabilities: capabilities,
            execution: execution,
            alternatives: table["alternatives"]?.stringArray ?? [],
            detect: detect,
            env: env,
            serve: serve,
            invoke: invoke,
            permissions: permissions,
            directory: directory)
    }
}
