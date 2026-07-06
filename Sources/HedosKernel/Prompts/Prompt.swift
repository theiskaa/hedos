import Foundation

public struct Prompt: Codable, Hashable, Sendable, Identifiable {
    public let id: String
    public var title: String
    public var body: String
    public var capability: Capability?
    public var createdAt: Date
    public var updatedAt: Date

    public init(
        id: String = UUID().uuidString,
        title: String,
        body: String,
        capability: Capability? = nil,
        createdAt: Date = Date(),
        updatedAt: Date = Date()
    ) {
        self.id = id
        self.title = title
        self.body = body
        self.capability = capability
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }

    enum CodingKeys: String, CodingKey {
        case id, title, body, capability, createdAt, updatedAt
    }

    public init(from decoder: any Decoder) throws {
        let now = Date()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self.init(id: "", title: "", body: "", createdAt: now, updatedAt: now)
            return
        }
        let id = container.lenient(String.self, .id) ?? ""
        let title = container.lenient(String.self, .title) ?? ""
        let body = container.lenient(String.self, .body) ?? ""
        let capability = container.lenient(Capability.self, .capability)
        let createdAt = container.lenient(Date.self, .createdAt) ?? now
        let updatedAt = container.lenient(Date.self, .updatedAt) ?? createdAt
        self.init(
            id: id, title: title, body: body, capability: capability,
            createdAt: createdAt, updatedAt: updatedAt)
    }

    public var placeholderNames: [String] {
        PromptPlaceholders.names(in: body)
    }

    public func resolvedBody(_ values: [String: String]) -> String {
        PromptPlaceholders.resolve(body, with: values)
    }

    func identified(as id: String) -> Prompt {
        Prompt(
            id: id, title: title, body: body, capability: capability,
            createdAt: createdAt, updatedAt: updatedAt)
    }
}

public enum PromptPlaceholders {
    public static func names(in body: String) -> [String] {
        var seen: Set<String> = []
        var names: [String] = []
        for match in body.matches(of: /\{([A-Za-z0-9_]+)\}/) {
            let name = String(match.1)
            if seen.insert(name).inserted {
                names.append(name)
            }
        }
        return names
    }

    public static func resolve(_ body: String, with values: [String: String]) -> String {
        var resolved = body
        for name in names(in: body) {
            guard let value = values[name] else { continue }
            resolved = resolved.replacingOccurrences(of: "{\(name)}", with: value)
        }
        return resolved
    }
}
