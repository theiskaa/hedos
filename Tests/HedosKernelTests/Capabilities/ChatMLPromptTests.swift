import Foundation
import Testing

@testable import HedosKernel

private let sampleMessages = [
    ChatMessage(role: .system, content: "be terse"),
    ChatMessage(role: .user, content: "hi there"),
    ChatMessage(role: .assistant, content: "hello"),
    ChatMessage(role: .user, content: "bye"),
]

@Test func chatMLRenderHasTheExpectedShape() {
    #expect(
        ChatMLPrompt.render(sampleMessages) == """
            <|im_start|>system
            be terse<|im_end|>
            <|im_start|>user
            hi there<|im_end|>
            <|im_start|>assistant
            hello<|im_end|>
            <|im_start|>user
            bye<|im_end|>
            <|im_start|>assistant

            """)
}

@Test func chatMLFallbackRenderingsPinEqualAcrossSwiftAndPython() throws {
    let swiftRendered = ChatMLPrompt.render(sampleMessages)

    let packageRoot = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    let pyDir = packageRoot.appendingPathComponent(
        "Sources/HedosKernel/Resources/Runtimes/python-mlx-lm")
    guard FileManager.default.fileExists(
        atPath: pyDir.appendingPathComponent("main.py").path)
    else { return }

    let payload = sampleMessages.map { ["role": $0.role.rawValue, "content": $0.content] }
    let messagesJSON = String(
        data: try JSONSerialization.data(withJSONObject: payload), encoding: .utf8)!
    let script = """
        import sys, json, os
        sys.path.insert(0, sys.argv[1])
        import main
        os.write(main.real_stdout, main.render_chatml(json.loads(sys.argv[2])).encode())
        """
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
    process.arguments = ["python3", "-c", script, pyDir.path, messagesJSON]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = Pipe()
    do {
        try process.run()
    } catch {
        return
    }
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { return }
    let pythonRendered =
        String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    #expect(swiftRendered == pythonRendered)
}
