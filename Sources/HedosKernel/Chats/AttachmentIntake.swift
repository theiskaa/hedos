import Foundation

public enum AttachmentIntake {
    public enum Verdict: Sendable, Equatable {
        case document(ChatAttachment)
        case binary
        case tooLarge(limit: Int)
    }

    public static let bytesPerToken = 4
    public static let minimumBudget = 4096
    public static let maximumBudget = 262_144
    public static let fallbackBudget = 32_768

    public static func documentBudget(effectiveWindow: Int?) -> Int {
        guard let effectiveWindow, effectiveWindow > 0 else { return fallbackBudget }
        let half = effectiveWindow * bytesPerToken / 2
        return min(max(half, minimumBudget), maximumBudget)
    }

    public static func classify(data: Data, filename: String, budgetBytes: Int) -> Verdict {
        guard data.count <= maximumBudget * 4 else { return .tooLarge(limit: budgetBytes) }
        let normalized = transcodedToUTF8(data)
        guard normalized.count <= budgetBytes else { return .tooLarge(limit: budgetBytes) }
        guard !normalized.contains(0) else { return .binary }
        let text = String(decoding: normalized, as: UTF8.self)
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

    private static func transcodedToUTF8(_ data: Data) -> Data {
        if data.starts(with: [0xFF, 0xFE, 0x00, 0x00]) || data.starts(with: [0x00, 0x00, 0xFE, 0xFF]) {
            return reencoded(data, as: .utf32) ?? data
        }
        if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) {
            return reencoded(data, as: .utf16) ?? data
        }
        if let endian = bomlessUTF16Endianness(of: data) {
            return reencoded(data, as: endian) ?? data
        }
        return data
    }

    private static func reencoded(_ data: Data, as encoding: String.Encoding) -> Data? {
        String(data: data, encoding: encoding).map { Data($0.utf8) }
    }

    private static func bomlessUTF16Endianness(of data: Data) -> String.Encoding? {
        let sniff = data.prefix(4096)
        guard sniff.count >= 8, data.count % 2 == 0 else { return nil }
        var zeros = (even: 0, odd: 0)
        var counts = (even: 0, odd: 0)
        for (index, byte) in sniff.enumerated() {
            if index % 2 == 0 {
                counts.even += 1
                if byte == 0 { zeros.even += 1 }
            } else {
                counts.odd += 1
                if byte == 0 { zeros.odd += 1 }
            }
        }
        if zeros.even == 0, zeros.odd * 10 >= counts.odd * 4 {
            return .utf16LittleEndian
        }
        if zeros.odd == 0, zeros.even * 10 >= counts.even * 4 {
            return .utf16BigEndian
        }
        return nil
    }
}
