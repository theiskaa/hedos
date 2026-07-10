import Foundation

enum SidecarWorkdir {
    static func defaultRoot() -> URL {
        Registry.defaultDirectory().appendingPathComponent("workdirs", isDirectory: true)
    }

    static func directory(root: URL, name: String) throws -> URL {
        let workdir = root.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: workdir, withIntermediateDirectories: true)
        return workdir
    }
}

enum SidecarBundle {
    static func require(_ name: String, runtimeID: RuntimeID) throws -> URL {
        guard let bundle = RuntimeBundle.directory(named: name),
            FileManager.default.fileExists(atPath: bundle.path)
        else { throw KernelError.bundleMissing(runtimeID: runtimeID) }
        return bundle
    }

    static func spec(
        runtimeID: RuntimeID, record: ModelRecord, bundle: URL, envDir: URL?,
        workdirRoot: URL, workdirName: String,
        extraArguments: [String] = [],
        cooperativeCancel: Bool = false
    ) throws -> SidecarSpec {
        guard let envDir else {
            throw KernelError.runtimeFailed("the \(runtimeID) environment was not prepared")
        }
        let paths = SidecarModelPaths.resolve(record)
        let workdir = try SidecarWorkdir.directory(root: workdirRoot, name: workdirName)
        return SidecarSpec(
            runtimeID: "\(runtimeID)#\(record.id)",
            executable: URL(fileURLWithPath: "/usr/bin/sandbox-exec"),
            arguments: SandboxArgv.build(
                envDir: envDir, bundle: bundle,
                modelSandboxRoot: URL(fileURLWithPath: paths.sandboxRoot), workdir: workdir,
                trailingArguments: ["--model", paths.snapshot] + extraArguments + [
                    "--workdir", workdir.path,
                ]),
            environment: ["PYTHONDONTWRITEBYTECODE": "1"],
            workingDirectory: workdir,
            readyTimeout: .seconds(600),
            cooperativeCancel: cooperativeCancel)
    }
}
