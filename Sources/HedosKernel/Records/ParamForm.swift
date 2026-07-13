import Foundation

extension ParamSpec {
    public var intRange: ClosedRange<Int>? {
        guard let range, range.count == 2,
            let lower = Self.intScalar(range[0]),
            let upper = Self.intScalar(range[1]),
            lower <= upper
        else { return nil }
        return lower...upper
    }

    public var doubleRange: ClosedRange<Double>? {
        guard let range, range.count == 2,
            let lower = Self.doubleScalar(range[0]),
            let upper = Self.doubleScalar(range[1]),
            lower <= upper
        else { return nil }
        return lower...upper
    }

    public var isOptional: Bool {
        defaultValue == nil || defaultValue == .null
    }

    public var doubleStep: Double? {
        doubleRange.map(Self.step(across:))
    }

    public static func step(across range: ClosedRange<Double>) -> Double {
        let width = range.upperBound - range.lowerBound
        guard width > 0, width.isFinite else { return 1 }
        return pow(10, (log10(width) - 2).rounded(.down))
    }

    public static func decimals(forStep step: Double) -> Int {
        guard step > 0, step < 1 else { return 0 }
        return Int((-log10(step)).rounded(.up))
    }

    public func normalized(_ value: JSONValue) -> JSONValue? {
        switch type {
        case .int:
            guard let raw = Self.intScalar(value) else { return nil }
            guard let range = intRange else { return .int(raw) }
            return .int(min(max(raw, range.lowerBound), range.upperBound))
        case .float:
            guard let raw = Self.doubleScalar(value) else { return nil }
            guard let range = doubleRange else { return .double(raw) }
            return .double(min(max(raw, range.lowerBound), range.upperBound))
        case .enumeration:
            guard case .string(let raw) = value, values?.contains(raw) == true else { return nil }
            return .string(raw)
        case .bool:
            guard case .bool(let raw) = value else { return nil }
            return .bool(raw)
        case .string:
            guard case .string(let raw) = value else { return nil }
            return .string(raw)
        }
    }

    private static func intScalar(_ value: JSONValue) -> Int? {
        switch value {
        case .int(let raw): raw
        case .double(let raw): Int(raw.rounded())
        default: nil
        }
    }

    private static func doubleScalar(_ value: JSONValue) -> Double? {
        switch value {
        case .int(let raw): Double(raw)
        case .double(let raw): raw
        default: nil
        }
    }
}

public struct ParamForm: Hashable, Sendable {
    public let schema: [ParamSpec]
    private var values: [String: JSONValue]

    public init(schema: [ParamSpec]) {
        self.schema = schema
        var seeded: [String: JSONValue] = [:]
        for spec in schema {
            if let fallback = spec.defaultValue, fallback != .null,
                let normalized = spec.normalized(fallback)
            {
                seeded[spec.key] = normalized
            }
        }
        self.values = seeded
    }

    public func spec(_ key: String) -> ParamSpec? {
        schema.first { $0.key == key }
    }

    public func value(_ key: String) -> JSONValue? {
        values[key]
    }

    public func int(_ key: String) -> Int? {
        guard case .int(let raw) = values[key] else { return nil }
        return raw
    }

    public func double(_ key: String) -> Double? {
        switch values[key] {
        case .double(let raw): raw
        case .int(let raw): Double(raw)
        default: nil
        }
    }

    public func string(_ key: String) -> String? {
        guard case .string(let raw) = values[key] else { return nil }
        return raw
    }

    public func bool(_ key: String) -> Bool? {
        guard case .bool(let raw) = values[key] else { return nil }
        return raw
    }

    public mutating func set(_ key: String, to value: JSONValue) {
        guard let spec = spec(key) else { return }
        guard let normalized = spec.normalized(value) else { return }
        values[key] = normalized
    }

    public mutating func clear(_ key: String) {
        values[key] = nil
    }

    public mutating func roll(_ key: String) {
        guard let spec = spec(key), spec.type == .int else { return }
        let range = spec.intRange ?? 0...Int(UInt32.max - 1)
        var fresh = JSONValue.int(Int.random(in: range))
        while range.count > 1 && fresh == values[key] {
            fresh = .int(Int.random(in: range))
        }
        values[key] = fresh
    }

    public mutating func load(_ params: JSONValue) {
        guard case .object(let fields) = params else { return }
        for spec in schema {
            if let raw = fields[spec.key], raw != .null, let normalized = spec.normalized(raw) {
                values[spec.key] = normalized
            } else if let fallback = spec.defaultValue, fallback != .null,
                let normalized = spec.normalized(fallback)
            {
                values[spec.key] = normalized
            } else {
                values[spec.key] = nil
            }
        }
    }

    public func payload(prompt: String) -> JSONValue {
        var fields: [String: JSONValue] = [:]
        for spec in schema {
            if let value = values[spec.key] {
                fields[spec.key] = value
            }
        }
        fields["prompt"] = .string(prompt)
        return .object(fields)
    }
}
