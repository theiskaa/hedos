import Foundation
import Testing

@testable import HedosKernel

@Test func maximalRecordRoundTripsThroughCodable() throws {
    let record = Fixtures.flux()
    let encoder = JSONEncoder()
    encoder.dateEncodingStrategy = .iso8601
    let decoder = JSONDecoder()
    decoder.dateDecodingStrategy = .iso8601

    let decoded = try decoder.decode(ModelRecord.self, from: try encoder.encode(record))
    #expect(decoded == record)
}

@Test func paramSpecEncodesDefaultKeyAsJSON() throws {
    let spec = ParamSpec(key: "steps", type: .int, defaultValue: .int(4))
    let json = String(data: try JSONEncoder().encode(spec), encoding: .utf8)!
    #expect(json.contains("\"default\""))
    #expect(json.contains("\"int\""))
}

@Test func stableIDIsDeterministicPerSource() {
    let a = Fixtures.flux()
    let b = Fixtures.flux()
    #expect(a.id == b.id)
    #expect(a.id.count == 16)

    let other = Fixtures.gguf()
    #expect(a.id != other.id)

    let asFolder = ModelSource(kind: .folder, path: a.source.path, repo: a.source.repo)
    #expect(ModelRecord.stableID(for: asFolder) != a.id)
}

@Test func jsonValueNumbersCompareNumerically() throws {
    #expect(JSONValue.double(0.0) == JSONValue.int(0))
    #expect(JSONValue.double(0.0).hashValue == JSONValue.int(0).hashValue)
    #expect(JSONValue.double(1.5) != JSONValue.int(1))
    let decoded = try JSONDecoder().decode(JSONValue.self, from: Data("0".utf8))
    #expect(decoded == .double(0.0))
}

@Test func jsonValueRoundTripsAllShapes() throws {
    let value: JSONValue = .object([
        "null": .null,
        "bool": .bool(true),
        "int": .int(42),
        "double": .double(1.5),
        "string": .string("hedos"),
        "array": .array([.int(1), .string("two")]),
    ])
    let decoded = try JSONDecoder().decode(JSONValue.self, from: try JSONEncoder().encode(value))
    #expect(decoded == value)
}

@Test func openIdentifiersAcceptUnknownValues() throws {
    let modality = try JSONDecoder().decode(Modality.self, from: Data("\"music\"".utf8))
    #expect(modality.rawValue == "music")
    let capability = try JSONDecoder().decode(Capability.self, from: Data("\"video\"".utf8))
    #expect(capability == Capability(rawValue: "video"))
}
