import Foundation
import Testing

@testable import HedosKernel

@Test func seedingInjectsASeedOnlyWhenAbsentOrNull() {
    let bare = PayloadSeeding.seeded(.object(["prompt": .string("hi")]))
    #expect(bare.objectValue?["seed"]?.intValue != nil)

    let null = PayloadSeeding.seeded(.object(["seed": .null]))
    #expect(null.objectValue?["seed"]?.intValue != nil)

    let pinned = PayloadSeeding.seeded(.object(["seed": .int(42)]))
    #expect(pinned.objectValue?["seed"] == .int(42))

    let scalar = PayloadSeeding.seeded(.string("not an object"))
    #expect(scalar == .string("not an object"))

    #expect(PayloadSeeding.seeded(.null).objectValue?["seed"]?.intValue != nil)
}

@Test func reseedingAlwaysPicksADifferentSeed() {
    let params: JSONValue = .object(["seed": .int(42), "prompt": .string("hi")])
    for _ in 0..<20 {
        let fresh = PayloadSeeding.reseeded(params)
        #expect(fresh.objectValue?["seed"] != .int(42))
        #expect(fresh.objectValue?["prompt"] == .string("hi"))
    }
}
