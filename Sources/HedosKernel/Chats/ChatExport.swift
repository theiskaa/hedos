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
    public static func markdown(_ transcript: ChatTranscript) -> String {
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
            lines.append("## \(turn.role.rawValue.capitalized)")
            lines.append("")
            lines.append(turn.content)
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
