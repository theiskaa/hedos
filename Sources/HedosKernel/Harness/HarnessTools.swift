import Foundation

public enum HarnessTools {
    public static let listEntriesCap = 500
    public static let readChunkCapBytes = 65_536
    public static let searchMatchesCap = 200
    public static let searchFileSizeCapBytes = 2_097_152

    public static let listDirectoryName = "list_directory"
    public static let readFileName = "read_file"
    public static let searchName = "search"

    public static func specs() -> [ToolSpec] {
        [
            ToolSpec(
                name: listDirectoryName,
                description:
                    "Lists files and directories inside the conversation's folder. "
                    + "Returns name, kind, and size per entry.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "path": .object([
                            "type": .string("string"),
                            "description": .string(
                                "Folder-relative path to list; defaults to the folder root."),
                        ]),
                        "depth": .object([
                            "type": .string("integer"),
                            "description": .string("How deep to descend, 1 to 3; default 1."),
                        ]),
                    ]),
                    "required": .array([]),
                ])),
            ToolSpec(
                name: readFileName,
                description:
                    "Reads a file inside the conversation's folder. Large files return "
                    + "a stated byte range; page with offset_bytes and length_bytes.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "path": .object([
                            "type": .string("string"),
                            "description": .string("Folder-relative path of the file to read."),
                        ]),
                        "offset_bytes": .object([
                            "type": .string("integer"),
                            "description": .string("Byte offset to start from; default 0."),
                        ]),
                        "length_bytes": .object([
                            "type": .string("integer"),
                            "description": .string(
                                "How many bytes to read; capped at 65536 per call."),
                        ]),
                    ]),
                    "required": .array([.string("path")]),
                ])),
            ToolSpec(
                name: searchName,
                description:
                    "Searches inside the conversation's folder: literal case-insensitive "
                    + "substring over file contents, or filename matching.",
                parameters: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "query": .object([
                            "type": .string("string"),
                            "description": .string("The literal text to search for."),
                        ]),
                        "glob": .object([
                            "type": .string("string"),
                            "description": .string(
                                "Optional filename filter, fnmatch-style, e.g. *.swift."),
                        ]),
                        "kind": .object([
                            "enum": .array([.string("content"), .string("filename")]),
                            "description": .string("What to match; default content."),
                        ]),
                    ]),
                    "required": .array([.string("query")]),
                ])),
        ]
    }

    public static func execute(_ call: ToolCall, place: String) async -> String {
        guard case .object(let arguments) = call.arguments else {
            return "This tool call carried no usable arguments."
        }
        do {
            switch call.name {
            case listDirectoryName:
                return try listDirectory(arguments, place: place)
            case readFileName:
                return try readFile(arguments, place: place)
            case searchName:
                return try search(arguments, place: place)
            default:
                return "There is no tool named \(call.name)."
            }
        } catch let error as HarnessError {
            return error.errorDescription ?? "The path was refused."
        } catch {
            return "The tool failed: \(error.localizedDescription)"
        }
    }

    static func listDirectory(_ arguments: [String: JSONValue], place: String) throws -> String {
        let requested = arguments["path"]?.stringValue ?? "."
        let depth = min(max(arguments["depth"]?.intValue ?? 1, 1), 3)
        let root = try PlaceBoundary.resolve(requested, in: place)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: root, isDirectory: &isDirectory),
            isDirectory.boolValue
        else {
            return "\(relative(root, place: place)) is not a directory that exists."
        }
        var lines: [String] = []
        var truncated = 0
        walk(root, depth: depth, place: place, lines: &lines, truncated: &truncated)
        if lines.isEmpty {
            return "\(relative(root, place: place)) is empty."
        }
        if truncated > 0 {
            lines.append("\(truncated) more entries omitted.")
        }
        return lines.joined(separator: "\n")
    }

    private static func walk(
        _ directory: String, depth: Int, place: String,
        lines: inout [String], truncated: inout Int
    ) {
        guard depth > 0 else { return }
        let entries =
            (try? FileManager.default.contentsOfDirectory(atPath: directory))?.sorted() ?? []
        for name in entries {
            let path = directory + "/" + name
            guard lines.count < listEntriesCap else {
                truncated += 1
                continue
            }
            let kind = entryKind(path)
            let size = fileSize(path)
            let sizeText = kind == "file" ? " — \(byteText(size))" : ""
            lines.append("\(kind) \(relative(path, place: place))\(sizeText)")
            if kind == "directory" {
                walk(path, depth: depth - 1, place: place, lines: &lines, truncated: &truncated)
            }
        }
    }

    static func readFile(_ arguments: [String: JSONValue], place: String) throws -> String {
        guard let requested = arguments["path"]?.stringValue else {
            return "read_file needs a path argument."
        }
        let path = try PlaceBoundary.resolve(requested, in: place)
        guard let handle = FileHandle(forReadingAtPath: path) else {
            return "\(relative(path, place: place)) does not exist or cannot be opened."
        }
        defer { try? handle.close() }
        let total = fileSize(path)
        let offset = max(arguments["offset_bytes"]?.intValue ?? 0, 0)
        let asked = arguments["length_bytes"]?.intValue ?? readChunkCapBytes
        let length = min(max(asked, 0), readChunkCapBytes)
        try handle.seek(toOffset: UInt64(offset))
        let data = (try? handle.read(upToCount: length)) ?? Data()

        let sniff = data.prefix(8192)
        if sniff.contains(0) {
            return
                "\(relative(path, place: place)) looks like a binary file "
                + "(\(byteText(total)) total); its bytes are not shown."
        }
        let text = String(decoding: data, as: UTF8.self)
        let header =
            "\(relative(path, place: place)) — bytes \(offset)..<\(offset + data.count) "
            + "of \(total) total"
        return header + "\n" + text
    }

    static func search(_ arguments: [String: JSONValue], place: String) throws -> String {
        guard let query = arguments["query"]?.stringValue, !query.isEmpty else {
            return "search needs a query argument."
        }
        let glob = arguments["glob"]?.stringValue
        let kind = arguments["kind"]?.stringValue ?? "content"
        let root = try PlaceBoundary.resolve(".", in: place)

        var matches: [String] = []
        var cutOff = false
        let enumerator = FileManager.default.enumerator(
            at: URL(fileURLWithPath: root), includingPropertiesForKeys: [.isRegularFileKey],
            options: [.skipsHiddenFiles])
        while let entry = enumerator?.nextObject() as? URL {
            if matches.count >= searchMatchesCap {
                cutOff = true
                break
            }
            let values = try? entry.resourceValues(
                forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
            guard values?.isRegularFile == true, values?.isSymbolicLink != true else {
                continue
            }
            guard let resolved = try? PlaceBoundary.resolve(entry.path, in: root),
                resolved == entry.path || resolved.hasPrefix(root + "/")
            else { continue }
            let name = entry.lastPathComponent
            if let glob, fnmatch(glob, name, 0) != 0 { continue }
            let relativePath = relative(entry.path, place: place)
            if kind == "filename" {
                if name.range(of: query, options: .caseInsensitive) != nil {
                    matches.append(relativePath)
                }
                continue
            }
            guard fileSize(entry.path) <= searchFileSizeCapBytes else { continue }
            guard let data = try? Data(contentsOf: entry), !data.prefix(8192).contains(0)
            else { continue }
            let content = String(decoding: data, as: UTF8.self)
            var lineNumber = 0
            for line in content.split(separator: "\n", omittingEmptySubsequences: false) {
                lineNumber += 1
                if matches.count >= searchMatchesCap {
                    cutOff = true
                    break
                }
                if line.range(of: query, options: .caseInsensitive) != nil {
                    let trimmedLine = line.trimmingCharacters(in: .whitespaces).prefix(200)
                    matches.append("\(relativePath):\(lineNumber): \(trimmedLine)")
                }
            }
        }
        if matches.isEmpty {
            return "No matches for \(query)."
        }
        var result = matches.joined(separator: "\n")
        if cutOff {
            result += "\nStopped at \(searchMatchesCap) matches."
        }
        return result
    }

    private static func entryKind(_ path: String) -> String {
        let attributes = try? FileManager.default.attributesOfItem(atPath: path)
        switch attributes?[.type] as? FileAttributeType {
        case .typeDirectory?: return "directory"
        case .typeSymbolicLink?: return "symlink"
        default: return "file"
        }
    }

    private static func fileSize(_ path: String) -> Int {
        let attributes = try? FileManager.default.attributesOfItem(atPath: path)
        return (attributes?[.size] as? NSNumber)?.intValue ?? 0
    }

    private static func relative(_ path: String, place: String) -> String {
        if path == place { return "." }
        if path.hasPrefix(place + "/") {
            return String(path.dropFirst(place.count + 1))
        }
        return path
    }

    static func byteText(_ bytes: Int) -> String {
        if bytes < 1024 { return "\(bytes) B" }
        if bytes < 1_048_576 {
            return String(format: "%.1f KB", Double(bytes) / 1024)
        }
        if bytes < 1_073_741_824 {
            return String(format: "%.1f MB", Double(bytes) / 1_048_576)
        }
        return String(format: "%.1f GB", Double(bytes) / 1_073_741_824)
    }
}
