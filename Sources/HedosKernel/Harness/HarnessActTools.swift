import Foundation

public enum HarnessActTools {
    public static let writeCapBytes = 2_097_152
    public static let commandTimeoutDefaultSeconds = 120
    public static let commandTimeoutCapSeconds = 600

    public static let writeFileName = "write_file"
    public static let editFileName = "edit_file"
    public static let runCommandName = "run_command"

    public static func specs() -> [ToolSpec] {
        [
            ToolSpec(
                name: writeFileName,
                description:
                    "Creates or replaces a file inside the conversation's folder. "
                    + "The user approves the exact change before it is written.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "path": .object([
                            "type": .string("string"),
                            "description": .string("Folder-relative path of the file to write."),
                        ]),
                        "content": .object([
                            "type": .string("string"),
                            "description": .string("The full new contents of the file."),
                        ]),
                    ]),
                    "required": .array([.string("path"), .string("content")]),
                ])),
            ToolSpec(
                name: editFileName,
                description:
                    "Replaces an exact string in a file inside the conversation's folder. "
                    + "The user approves the exact change before it is written.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "path": .object([
                            "type": .string("string"),
                            "description": .string("Folder-relative path of the file to edit."),
                        ]),
                        "old": .object([
                            "type": .string("string"),
                            "description": .string("The exact text to replace."),
                        ]),
                        "new": .object([
                            "type": .string("string"),
                            "description": .string("The text to replace it with."),
                        ]),
                        "replace_all": .object([
                            "type": .string("boolean"),
                            "description": .string(
                                "Replace every occurrence; default false (old must be unique)."),
                        ]),
                    ]),
                    "required": .array([.string("path"), .string("old"), .string("new")]),
                ])),
            ToolSpec(
                name: runCommandName,
                description:
                    "Runs a command inside the conversation's folder, sandboxed with no network. "
                    + "The command is an array of arguments (no shell). The user approves the exact "
                    + "command before it runs.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "argv": .object([
                            "type": .string("array"),
                            "items": .object(["type": .string("string")]),
                            "description": .string(
                                "The command and its arguments, one string per element."),
                        ]),
                        "timeout_seconds": .object([
                            "type": .string("integer"),
                            "description": .string(
                                "Wall-clock limit in seconds; default 120, capped at 600."),
                        ]),
                    ]),
                    "required": .array([.string("argv")]),
                ])),
        ]
    }

    public static func execute(
        _ call: ToolCall, place: String, context: HarnessActContext
    ) async -> String {
        guard case .object(let arguments) = call.arguments else {
            return "This tool call carried no usable arguments."
        }
        switch call.name {
        case writeFileName:
            return await writeFile(arguments, place: place, context: context)
        case editFileName:
            return await editFile(arguments, place: place, context: context)
        case runCommandName:
            return await runCommand(arguments, place: place, context: context)
        default:
            return "There is no tool named \(call.name)."
        }
    }

    static func isActTool(_ name: String) -> Bool {
        name == writeFileName || name == editFileName || name == runCommandName
    }

    private static func requestConsent(
        _ kind: ConsentRequest.Kind, context: HarnessActContext, toolName: String
    ) async -> ConsentDecision {
        var overwritesForeignFile = false
        if case .write(_, _, let foreign) = kind { overwritesForeignFile = foreign != nil }
        if !overwritesForeignFile,
            await context.state.isGranted(toolName, session: context.sessionID)
        {
            return .approved(dontAskAgain: true)
        }
        let request = ConsentRequest(
            id: UUID().uuidString, sessionID: context.sessionID, toolName: toolName, kind: kind)
        let decision = await context.ask(request)
        if case .approved(true) = decision {
            await context.state.grant(toolName, session: context.sessionID)
        }
        return decision
    }

    private struct PathRefusal: Error { let message: String }

    private static func resolve(_ requested: String, in place: String)
        -> Result<String, PathRefusal>
    {
        do {
            return .success(try PlaceBoundary.resolve(requested, in: place))
        } catch let error as HarnessError {
            return .failure(PathRefusal(message: error.errorDescription ?? "The path was refused."))
        } catch {
            return .failure(
                PathRefusal(message: "The path was refused: \(error.localizedDescription)"))
        }
    }

    static func writeFile(
        _ arguments: [String: JSONValue], place: String, context: HarnessActContext
    ) async -> String {
        guard let requested = arguments["path"]?.stringValue else {
            return "write_file needs a path argument."
        }
        guard let content = arguments["content"]?.stringValue else {
            return "write_file needs a content argument."
        }
        guard content.utf8.count <= writeCapBytes else {
            return "write_file content is larger than the \(writeCapBytes)-byte cap."
        }
        let path: String
        switch resolve(requested, in: place) {
        case .success(let resolved): path = resolved
        case .failure(let refusal): return refusal.message
        }
        let shown = PlacePaths.relative(path, place: place)
        let before = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
        let existed = FileManager.default.fileExists(atPath: path)
        let foreign: String?
        if existed, !(await context.state.wasCreatedThisSession(path, session: context.sessionID)) {
            let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size]) as? Int ?? 0
            foreign = "\(shown) (\(HarnessTools.byteText(size)))"
        } else {
            foreign = nil
        }
        let diff = Diff.unified(from: before, to: content, path: shown)
        let decision = await requestConsent(
            .write(path: shown, diff: diff, overwritesForeignFile: foreign),
            context: context, toolName: writeFileName)
        guard case .approved = decision else {
            return "The user declined this write to \(shown)."
        }
        if Task.isCancelled {
            return "This write to \(shown) was cancelled before it ran."
        }
        let currentContent = (try? String(contentsOfFile: path, encoding: .utf8)) ?? ""
        let existsNow = FileManager.default.fileExists(atPath: path)
        if currentContent != before || existsNow != existed {
            return "\(shown) changed while waiting for approval; nothing was written. Read it again and retry."
        }
        let directory = (path as NSString).deletingLastPathComponent
        try? FileManager.default.createDirectory(
            atPath: directory, withIntermediateDirectories: true)
        do {
            try content.write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            return "The write to \(shown) failed: \(error.localizedDescription)"
        }
        await context.state.recordCreated(path, session: context.sessionID)
        return "wrote \(shown)\n\(diff)"
    }

    static func editFile(
        _ arguments: [String: JSONValue], place: String, context: HarnessActContext
    ) async -> String {
        guard let requested = arguments["path"]?.stringValue else {
            return "edit_file needs a path argument."
        }
        guard let old = arguments["old"]?.stringValue, let new = arguments["new"]?.stringValue
        else {
            return "edit_file needs old and new arguments."
        }
        var replaceAll = false
        if case .bool(let flag)? = arguments["replace_all"] { replaceAll = flag }
        let path: String
        switch resolve(requested, in: place) {
        case .success(let resolved): path = resolved
        case .failure(let refusal): return refusal.message
        }
        let shown = PlacePaths.relative(path, place: place)
        guard let before = try? String(contentsOfFile: path, encoding: .utf8) else {
            return "\(shown) does not exist or cannot be read as text."
        }
        let occurrences = before.components(separatedBy: old).count - 1
        guard occurrences > 0 else {
            return "The exact text to replace was not found in \(shown)."
        }
        if !replaceAll, occurrences > 1 {
            return
                "The text to replace appears \(occurrences) times in \(shown); make it unique or set replace_all."
        }
        let after: String
        if replaceAll {
            after = before.replacingOccurrences(of: old, with: new)
        } else {
            if let range = before.range(of: old) {
                after = before.replacingCharacters(in: range, with: new)
            } else {
                after = before
            }
        }
        guard after.utf8.count <= writeCapBytes else {
            return "The edited file would exceed the \(writeCapBytes)-byte cap."
        }
        let foreign: String?
        if !(await context.state.wasCreatedThisSession(path, session: context.sessionID)) {
            let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size]) as? Int ?? 0
            foreign = "\(shown) (\(HarnessTools.byteText(size)))"
        } else {
            foreign = nil
        }
        let diff = Diff.unified(from: before, to: after, path: shown)
        let decision = await requestConsent(
            .write(path: shown, diff: diff, overwritesForeignFile: foreign),
            context: context, toolName: editFileName)
        guard case .approved = decision else {
            return "The user declined this edit to \(shown)."
        }
        if Task.isCancelled {
            return "This edit to \(shown) was cancelled before it ran."
        }
        if (try? String(contentsOfFile: path, encoding: .utf8)) != before {
            return "\(shown) changed while waiting for approval; nothing was written. Read it again and retry."
        }
        do {
            try after.write(toFile: path, atomically: true, encoding: .utf8)
        } catch {
            return "The edit to \(shown) failed: \(error.localizedDescription)"
        }
        await context.state.recordCreated(path, session: context.sessionID)
        return "edited \(shown)\n\(diff)"
    }

    static func runCommand(
        _ arguments: [String: JSONValue], place: String, context: HarnessActContext
    ) async -> String {
        guard case .array(let argvValues)? = arguments["argv"] else {
            return "run_command needs an argv array of strings."
        }
        let argv = argvValues.compactMap { $0.stringValue }
        guard !argv.isEmpty, argv.count == argvValues.count else {
            return "run_command needs a non-empty argv array of strings."
        }
        let requested = arguments["timeout_seconds"]?.intValue ?? commandTimeoutDefaultSeconds
        let timeout = min(max(requested, 1), commandTimeoutCapSeconds)

        let decision = await requestConsent(
            .command(argv: argv, timeoutSeconds: timeout),
            context: context, toolName: runCommandName)
        guard case .approved = decision else {
            return "The user declined running: \(argv.joined(separator: " "))"
        }
        if Task.isCancelled {
            return "Running \(argv.joined(separator: " ")) was cancelled before it started."
        }

        guard
            let profile = RuntimeBundle.directory(named: "generic")?
                .appendingPathComponent("harness.sb")
        else {
            return "The command sandbox profile is missing."
        }
        let canonicalPlace = SandboxArgv.canonicalPath(URL(fileURLWithPath: place))
        let tmp = SandboxArgv.canonicalPath(FileManager.default.temporaryDirectory)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/sandbox-exec")
        process.arguments =
            ["-f", profile.path, "-D", "PLACE=\(canonicalPlace)", "-D", "TMP=\(tmp)"] + argv
        process.currentDirectoryURL = URL(fileURLWithPath: place)
        let host = ProcessInfo.processInfo.environment
        var environment = ["PATH": host["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"]
        if let home = host["HOME"] { environment["HOME"] = home }
        if let lang = host["LANG"] { environment["LANG"] = lang }
        environment["TMPDIR"] = tmp
        process.environment = environment
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        let drain = PipeDrain(stdout: stdout, stderr: stderr) {
            ProcessContainment.terminateProcessTree(process)
        }
        do {
            try process.run()
        } catch {
            return "$ \(argv.joined(separator: " "))\nfailed to start: \(error.localizedDescription)"
        }
        let timedOut = TimeoutFlag()
        let timeoutTask = Task {
            try? await Task.sleep(for: .seconds(timeout))
            guard !Task.isCancelled else { return }
            await timedOut.fire()
            ProcessContainment.terminateProcessTree(process)
            drain.cancel()
        }
        let (outData, errData) = await withTaskCancellationHandler {
            await drain.collect(process: process)
        } onCancel: {
            ProcessContainment.terminateProcessTree(process)
            timeoutTask.cancel()
            drain.cancel()
        }
        timeoutTask.cancel()

        let header = "$ \(argv.joined(separator: " "))"
        if await timedOut.didFire {
            return "\(header)\nstopped: the command ran longer than \(timeout)s and was killed."
        }
        var lines = [header, "exit: \(process.terminationStatus)"]
        let streamCap = ChatFlow.toolResultContextBudgetBytes / 2 - 256
        let out = truncated(String(decoding: outData, as: UTF8.self), cap: streamCap)
        let err = truncated(String(decoding: errData, as: UTF8.self), cap: streamCap)
        if !out.isEmpty { lines.append("stdout:\n\(out)") }
        if !err.isEmpty { lines.append("stderr:\n\(err)") }
        return lines.joined(separator: "\n")
    }

    static func truncated(_ text: String, cap: Int) -> String {
        let clip = TextBudget.clip(text, to: cap)
        guard clip.overflowed else { return text }
        return String(clip.kept) + "\n[output truncated at \(cap) bytes]"
    }
}
