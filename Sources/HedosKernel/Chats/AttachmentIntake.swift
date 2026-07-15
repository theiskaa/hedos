import Foundation

public enum AttachmentIntake {
    public enum Verdict: Sendable, Equatable {
        case document(ChatAttachment)
        case binary
        case tooLarge(limit: Int)
    }

    public static let bytesPerToken = 4
    public static let minimumBudget = 16_384
    public static let maximumBudget = 262_144
    public static let fallbackBudget = 32_768

    public static func documentBudget(contextLength: Int?) -> Int {
        guard let contextLength, contextLength > 0 else { return fallbackBudget }
        let half = contextLength * bytesPerToken / 2
        return min(max(half, minimumBudget), maximumBudget)
    }

    public static func classify(data: Data, filename: String, budgetBytes: Int) -> Verdict {
        guard data.count <= budgetBytes else { return .tooLarge(limit: budgetBytes) }
        guard !data.prefix(8192).contains(0) else { return .binary }
        let text = String(decoding: data, as: UTF8.self)
        let stored = Data(text.utf8)
        let originalExtension = (filename as NSString).pathExtension.lowercased()
        let ext =
            AttachmentStore.textExtensions.contains(originalExtension)
            ? originalExtension : "txt"
        let mimeType = AttachmentStore.mimeType(for: ext)
        let slug = AttachmentStore.slug(filename)
        let name = slug.isEmpty ? "attachment.\(ext)" : "\(slug).\(ext)"
        return .document(
            ChatAttachment(kind: .document, data: stored, mimeType: mimeType, name: name))
    }
}
