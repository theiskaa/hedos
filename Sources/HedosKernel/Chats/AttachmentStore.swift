import CryptoKit
import Foundation

struct AttachmentStore: Sendable {
    let directory: URL

    init(directory: URL) {
        self.directory = directory
    }

    static func ref(for data: Data, mimeType: String, name: String? = nil) -> String {
        let digest = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let named = name.map { ($0 as NSString).pathExtension.lowercased() } ?? ""
        let ext = textExtensions.contains(named) ? named : fileExtension(for: mimeType)
        let slug = name.map(Self.slug) ?? ""
        return slug.isEmpty ? "\(digest).\(ext)" : "\(digest).\(slug).\(ext)"
    }

    static func slug(_ name: String) -> String {
        let stem = (name as NSString).deletingPathExtension.lowercased()
        var out = ""
        var dashed = true
        for scalar in stem.unicodeScalars {
            if CharacterSet.alphanumerics.contains(scalar), scalar.isASCII {
                out.unicodeScalars.append(scalar)
                dashed = false
            } else if !dashed {
                out.append("-")
                dashed = true
            }
            if out.count >= 40 { break }
        }
        while out.hasSuffix("-") { out.removeLast() }
        return out
    }

    func store(_ attachments: [ChatAttachment]) throws -> [String] {
        guard !attachments.isEmpty else { return [] }
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        var refs: [String] = []
        for attachment in attachments {
            let ref = Self.ref(
                for: attachment.data, mimeType: attachment.mimeType, name: attachment.name)
            let url = directory.appendingPathComponent(ref)
            if !FileManager.default.fileExists(atPath: url.path) {
                try attachment.data.write(to: url, options: .atomic)
            }
            refs.append(ref)
        }
        return refs
    }

    func load(_ refs: [String]) -> [ChatAttachment] {
        loadPairs(refs).map(\.attachment)
    }

    func loadPairs(_ refs: [String]) -> [(ref: String, attachment: ChatAttachment)] {
        refs.compactMap { ref in
            guard Self.isSafeRef(ref),
                let data = try? Data(contentsOf: directory.appendingPathComponent(ref))
            else { return nil }
            let ext = (ref as NSString).pathExtension
            let mimeType = Self.mimeType(for: ext)
            if Self.imageExtensions.contains(ext.lowercased()) {
                return (ref, ChatAttachment(kind: .image, data: data, mimeType: mimeType))
            }
            if Self.textExtensions.contains(ext.lowercased()) {
                let stem = (ref as NSString).deletingPathExtension
                let slug = (stem as NSString).pathExtension
                let name = slug.isEmpty ? nil : "\(slug).\(ext)"
                return (
                    ref,
                    ChatAttachment(kind: .document, data: data, mimeType: mimeType, name: name)
                )
            }
            return (ref, ChatAttachment(kind: .image, data: data, mimeType: mimeType))
        }
    }

    static func isSafeRef(_ ref: String) -> Bool {
        !ref.isEmpty && ref == (ref as NSString).lastPathComponent && ref != "."
            && ref != ".." && !ref.hasPrefix(".")
    }

    static let imageExtensions: Set<String> = ["png", "jpg", "jpeg", "webp", "gif"]

    static let textExtensions: Set<String> = [
        "txt", "md", "json", "csv", "yaml", "yml", "xml", "html", "log", "toml",
        "swift", "py", "js", "ts", "tsx", "jsx", "sh", "rb", "go", "rs", "c", "h",
        "cpp", "hpp", "css", "sql", "conf", "ini",
    ]

    static func fileExtension(for mimeType: String) -> String {
        switch mimeType {
        case "image/png": return "png"
        case "image/jpeg", "image/jpg": return "jpg"
        case "image/webp": return "webp"
        case "image/gif": return "gif"
        case "text/plain": return "txt"
        case "text/markdown": return "md"
        case "application/json", "text/json": return "json"
        case "text/csv": return "csv"
        case "application/x-yaml", "text/yaml", "application/yaml": return "yaml"
        case "application/xml", "text/xml": return "xml"
        case "text/html": return "html"
        case "application/toml", "text/toml": return "toml"
        default: return "bin"
        }
    }

    static func mimeType(for fileExtension: String) -> String {
        switch fileExtension.lowercased() {
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "webp": return "image/webp"
        case "gif": return "image/gif"
        case "txt", "log", "swift", "py", "js", "ts", "tsx", "jsx", "sh", "rb", "go",
            "rs", "c", "h", "cpp", "hpp", "css", "sql", "conf", "ini":
            return "text/plain"
        case "md": return "text/markdown"
        case "json": return "application/json"
        case "csv": return "text/csv"
        case "yaml", "yml": return "application/x-yaml"
        case "xml": return "application/xml"
        case "html": return "text/html"
        case "toml": return "application/toml"
        default: return "application/octet-stream"
        }
    }
}
