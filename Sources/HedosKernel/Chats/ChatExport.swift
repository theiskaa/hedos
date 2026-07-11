import Foundation

public enum ChatExportError: Error, Sendable, LocalizedError {
    case malformedArchive(String)

    public var errorDescription: String? {
        switch self {
        case .malformedArchive(let description):
            "Chat archive cannot be read: \(description)"
        }
    }
}

public enum ChatExport {
    public static func markdown(
        _ transcript: ChatTranscript, includeThinking: Bool = false
    ) -> String {
        var lines: [String] = []
        lines.append("# \(transcript.session.title)")
        lines.append("")
        var metadata: [String] = []
        if let modelID = transcript.session.modelID {
            metadata.append("model: \(modelID)")
        }
        metadata.append("created: \(timestamp(transcript.session.createdAt))")
        metadata.append("updated: \(timestamp(transcript.session.updatedAt))")
        lines.append(metadata.joined(separator: " · "))
        for turn in transcript.turns where turn.supersededBy == nil {
            lines.append("")
            var heading = "## \(turn.role.rawValue.capitalized)"
            if turn.role == .assistant, let modelID = turn.modelID {
                heading += " · \(modelID)"
            }
            if turn.interrupted { heading += " (interrupted)" }
            lines.append(heading)
            lines.append("")
            if includeThinking, let thinking = turn.thinking, !thinking.isEmpty {
                lines.append("**Thinking**")
                lines.append("")
                for line in thinking.split(separator: "\n", omittingEmptySubsequences: false) {
                    lines.append("> \(line)")
                }
                lines.append("")
            }
            if !turn.content.isEmpty {
                lines.append(turn.content)
            }
            for ref in turn.artifactRefs {
                lines.append("*(generated artifact: \(ref))*")
            }
        }
        lines.append("")
        return lines.joined(separator: "\n")
    }

    public static func json(_ transcript: ChatTranscript) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .prettyPrinted]
        return try encoder.encode(transcript)
    }

    public static func decode(_ data: Data) throws -> ChatTranscript {
        do {
            return try JSONDecoder().decode(ChatTranscript.self, from: data)
        } catch {
            throw ChatExportError.malformedArchive(String(describing: error))
        }
    }

    private static func timestamp(_ date: Date) -> String {
        date.formatted(.iso8601)
    }
}
