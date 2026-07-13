import Foundation

enum SandboxArgv {
    static func canonicalPath(_ url: URL) -> String {
        CanonicalPath.of(url)
    }

    static func build(
        envDir: URL,
        bundle: URL,
        modelSandboxRoot: URL,
        workdir: URL,
        trailingArguments: [String]
    ) -> [String] {
        let fm = FileManager.default
        let python = envDir.appendingPathComponent("bin/python")
        let realPython = URL(fileURLWithPath: canonicalPath(python))
        let uvPythonRoot = realPython.deletingLastPathComponent().deletingLastPathComponent()
        let tmp = URL(fileURLWithPath: canonicalPath(fm.temporaryDirectory))
        let darwinCache = tmp.deletingLastPathComponent().appendingPathComponent("C")

        return [
            "-f", bundle.appendingPathComponent("sandbox.sb").path,
            "-D", "VENV=\(canonicalPath(envDir))",
            "-D", "UVPY=\(uvPythonRoot.path)",
            "-D", "MODEL=\(canonicalPath(modelSandboxRoot))",
            "-D", "WORKDIR=\(canonicalPath(workdir))",
            "-D", "RESOURCES=\(bundle.path)",
            "-D", "TMP=\(tmp.path)",
            "-D", "CACHE=\(darwinCache.path)",
            python.path,
            bundle.appendingPathComponent("main.py").path,
        ] + trailingArguments
    }
}
