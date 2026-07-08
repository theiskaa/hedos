import Foundation

public struct GatewayRequest: Sendable {
    public var method: String
    public var path: String
    public var query: [String: String]
    public var headers: [(name: String, value: String)]
    public var body: Data

    public init(
        method: String, uri: String, headers: [(name: String, value: String)], body: Data
    ) {
        self.method = method.uppercased()
        self.headers = headers
        self.body = body
        let components = URLComponents(string: uri)
        var path = components?.path ?? uri
        if path.count > 1, path.hasSuffix("/") { path.removeLast() }
        self.path = path.isEmpty ? "/" : path
        var query: [String: String] = [:]
        for item in components?.queryItems ?? [] {
            query[item.name] = item.value ?? ""
        }
        self.query = query
    }

    public func header(_ name: String) -> String? {
        headers.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?.value
    }

    public var bearerToken: String? {
        if let authorization = header("Authorization") {
            let parts = authorization.split(separator: " ", maxSplits: 1)
            if parts.count == 2, parts[0].caseInsensitiveCompare("Bearer") == .orderedSame {
                return String(parts[1]).trimmingCharacters(in: .whitespaces)
            }
        }
        return header("x-api-key")?.trimmingCharacters(in: .whitespaces)
    }

    public func decodedJSON() throws -> [String: Any] {
        guard !body.isEmpty,
            let object = try? JSONSerialization.jsonObject(with: body),
            let dictionary = object as? [String: Any]
        else {
            throw GatewayError(.badRequest, "request body must be a JSON object")
        }
        return dictionary
    }
}
