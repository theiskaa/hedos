import Foundation
import Testing

@testable import HedosKernel

@Test func bundledPythonEntrypointsCompile() throws {
    for name in [
        "python-mlx-audio", "python-mflux", "python-diffusers", "python-mlx-lm",
        "python-whisper-cpp", "python-mlx-vlm", "python-embeddings",
    ] {
        guard let bundle = RuntimeBundle.directory(named: name) else { continue }
        let main = bundle.appendingPathComponent("main.py")
        guard FileManager.default.fileExists(atPath: main.path) else { continue }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["python3", "-m", "py_compile", main.path]
        let stderr = Pipe()
        process.standardError = stderr
        try process.run()
        let diagnostics = String(
            decoding: try stderr.fileHandleForReading.readToEnd() ?? Data(), as: UTF8.self)
        process.waitUntilExit()
        #expect(process.terminationStatus == 0, "\(name)/main.py failed to compile: \(diagnostics)")
    }
}
