import Foundation

public enum InstallReference {
    static let huggingFaceHosts = ["huggingface.co/", "www.huggingface.co/", "hf.co/"]
    static let ollamaHosts = ["ollama.com/", "www.ollama.com/", "registry.ollama.ai/"]
    static let huggingFaceSubpaths: Set<String> = [
        "tree", "blob", "resolve", "commit", "commits", "discussions", "blame", "raw",
    ]
    static let huggingFaceReservedRoots: Set<String> = [
        "datasets", "spaces", "collections", "models", "blog", "docs", "papers",
        "tasks", "posts", "pricing", "settings", "organizations", "learn", "chat",
    ]

    public static func isHuggingFaceLink(_ raw: String) -> Bool {
        guard let text = cleaned(raw) else { return false }
        return huggingFaceHosts.contains { text.lowercased().hasPrefix($0) }
    }

    public static func isOllamaLink(_ raw: String) -> Bool {
        guard let text = cleaned(raw) else { return false }
        return ollamaHosts.contains { text.lowercased().hasPrefix($0) }
    }

    public static func huggingFaceRepo(from raw: String) -> String? {
        guard var text = cleaned(raw) else { return nil }
        text = stripped(text, hosts: huggingFaceHosts)
        guard !text.contains("://"), !text.contains(":") else { return nil }
        let components = text.split(separator: "/", omittingEmptySubsequences: false)
            .map(String.init)
        guard components.count >= 2 else { return nil }
        let org = components[0]
        let name = components[1]
        guard !org.isEmpty, !name.isEmpty,
            !huggingFaceReservedRoots.contains(org.lowercased())
        else { return nil }
        if components.count > 2 {
            guard raw.lowercased().contains("hf.co") || raw.lowercased().contains("huggingface.co"),
                let next = components.dropFirst(2).first,
                next.isEmpty || huggingFaceSubpaths.contains(next.lowercased())
            else { return nil }
        }
        return "\(org)/\(name)"
    }

    public static func ollamaTag(from raw: String) -> String? {
        tag(from: raw, requireExplicitTagForNamespaced: true)
    }

    public static func ollamaInstallTag(from raw: String) -> String? {
        tag(from: raw, requireExplicitTagForNamespaced: false)
    }

    private static func tag(
        from raw: String, requireExplicitTagForNamespaced: Bool
    ) -> String? {
        guard var text = cleaned(raw) else { return nil }
        let isLink = ollamaHosts.contains { text.lowercased().hasPrefix($0) }
        text = stripped(text, hosts: ollamaHosts)
        guard !text.contains("://") else { return nil }
        if isLink {
            var components = text.split(separator: "/", omittingEmptySubsequences: false)
                .map(String.init)
            if components.first?.lowercased() == "library" {
                components = Array(components.dropFirst().prefix(1))
            } else {
                components = Array(components.prefix(2))
            }
            guard !components.isEmpty else { return nil }
            text = components.joined(separator: "/")
            guard shaped(text, requireExplicitTagForNamespaced: false) else { return nil }
            return text
        }
        if text.lowercased().hasPrefix("library/") {
            text = String(text.dropFirst("library/".count))
        }
        guard shaped(text, requireExplicitTagForNamespaced: requireExplicitTagForNamespaced)
        else { return nil }
        return text
    }

    public static func normalizedTag(_ reference: String) -> String {
        (reference.contains(":") ? reference : reference + ":latest").lowercased()
    }

    public static func normalized(
        provider: InstallProviderID, reference: String
    ) -> String {
        provider == .ollama ? normalizedTag(reference) : reference.lowercased()
    }

    static func isOllamaTagShaped(_ reference: String) -> Bool {
        shaped(reference, requireExplicitTagForNamespaced: true)
    }

    private static func shaped(
        _ reference: String, requireExplicitTagForNamespaced: Bool
    ) -> Bool {
        guard !reference.isEmpty,
            reference.rangeOfCharacter(from: .whitespacesAndNewlines) == nil,
            !reference.contains("://")
        else { return false }
        let components = reference.split(separator: "/", omittingEmptySubsequences: false)
        guard components.count <= 2, components.allSatisfy({ !$0.isEmpty }) else { return false }
        guard let name = components.last else { return false }
        let nameParts = name.split(separator: ":", omittingEmptySubsequences: false)
        guard nameParts.count <= 2, nameParts.allSatisfy({ !$0.isEmpty }) else { return false }
        if components.count == 2 {
            return !requireExplicitTagForNamespaced || name.contains(":")
        }
        return true
    }

    private static func cleaned(_ raw: String) -> String? {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty,
            text.rangeOfCharacter(from: .whitespacesAndNewlines) == nil
        else { return nil }
        for scheme in ["https://", "http://"] where text.lowercased().hasPrefix(scheme) {
            text = String(text.dropFirst(scheme.count))
        }
        if let stop = text.firstIndex(where: { $0 == "?" || $0 == "#" }) {
            text = String(text[..<stop])
        }
        while text.hasSuffix("/") {
            text = String(text.dropLast())
        }
        return text.isEmpty ? nil : text
    }

    private static func stripped(_ text: String, hosts: [String]) -> String {
        for host in hosts where text.lowercased().hasPrefix(host) {
            return String(text.dropFirst(host.count))
        }
        return text
    }
}
