import Foundation

enum WireParamDecoding {
    static func stop(_ raw: Any?, maxCount: Int? = nil) throws -> JSONValue? {
        guard let raw else { return nil }
        if let single = raw as? String {
            return .array([.string(single)])
        }
        if let list = raw as? [Any] {
            if let maxCount, list.count > maxCount {
                throw GatewayError(
                    .badRequest, "stop accepts at most \(maxCount) sequences",
                    code: "unsupported_parameter")
            }
            var strings: [JSONValue] = []
            for item in list {
                guard let text = item as? String else {
                    throw GatewayError(.badRequest, "stop must be a string or array of strings")
                }
                strings.append(.string(text))
            }
            return .array(strings)
        }
        throw GatewayError(.badRequest, "stop must be a string or array of strings")
    }

    static func rejectUnknownKeys(
        _ body: [String: Any], honored: Set<String>, label: String
    ) throws {
        for key in body.keys.sorted() where !honored.contains(key) {
            throw GatewayError(
                .badRequest, "the \(label) '\(key)' is not supported",
                code: "unsupported_parameter")
        }
    }
}
