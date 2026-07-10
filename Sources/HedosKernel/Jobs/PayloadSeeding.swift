import Foundation

enum PayloadSeeding {
    static func seeded(_ payload: JSONValue) -> JSONValue {
        guard var fields = seedableFields(payload) else { return payload }
        if let seed = fields["seed"], seed != .null { return .object(fields) }
        fields["seed"] = .int(randomSeed())
        return .object(fields)
    }

    static func reseeded(_ params: JSONValue) -> JSONValue {
        guard var fields = seedableFields(params) else { return params }
        let previous = fields["seed"]
        var fresh = JSONValue.int(randomSeed())
        while fresh == previous {
            fresh = .int(randomSeed())
        }
        fields["seed"] = fresh
        return .object(fields)
    }

    private static func seedableFields(_ payload: JSONValue) -> [String: JSONValue]? {
        switch payload {
        case .object(let fields): fields
        case .null: [:]
        default: nil
        }
    }

    private static func randomSeed() -> Int {
        Int.random(in: 0..<Int(UInt32.max))
    }
}
