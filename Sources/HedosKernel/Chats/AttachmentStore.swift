import CryptoKit
import Foundation

public struct AttachmentStore: Sendable {
    let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    public func store(_ attachments: [ChatAttachment]) throws -> [String] {
        guard !attachments.isEmpty else { return [] }
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        var refs: [String] = []
        for attachment in attachments {
            let digest = SHA256.hash(data: attachment.data)
                .map { String(format: "%02x", $0) }.joined()
            let ref = "\(digest).\(Self.fileExtension(for: attachment.mimeType))"
            let url = directory.appendingPathComponent(ref)
            if !FileManager.default.fileExists(atPath: url.path) {
                try attachment.data.write(to: url, options: .atomic)
            }
            refs.append(ref)
        }
        return refs
    }

    public func load(_ refs: [String]) -> [ChatAttachment] {
        refs.compactMap { ref in
            guard Self.isSafeRef(ref),
                let data = try? Data(contentsOf: directory.appendingPathComponent(ref))
            else { return nil }
            let ext = (ref as NSString).pathExtension
            return ChatAttachment(kind: .image, data: data, mimeType: Self.mimeType(for: ext))
        }
    }

    static func isSafeRef(_ ref: String) -> Bool {
        !ref.isEmpty && ref == (ref as NSString).lastPathComponent && ref != "."
            && ref != ".." && !ref.hasPrefix(".")
    }

    static func fileExtension(for mimeType: String) -> String {
        switch mimeType {
        case "image/png": return "png"
        case "image/jpeg", "image/jpg": return "jpg"
        case "image/webp": return "webp"
        case "image/gif": return "gif"
        default: return "bin"
        }
    }

    static func mimeType(for fileExtension: String) -> String {
        switch fileExtension.lowercased() {
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "webp": return "image/webp"
        case "gif": return "image/gif"
        default: return "application/octet-stream"
        }
    }
}
