import Foundation
import Testing

@testable import HedosKernel

private func fluxForm() -> ParamForm {
    ParamForm(schema: Fixtures.flux().params)
}

@Test func formSeedsDefaultsFromSchemaAndLeavesOptionalsUnset() {
    let form = fluxForm()
    #expect(form.int("steps") == 4)
    #expect(form.double("guidance") == 4.0)
    #expect(form.string("size") == "1024x1024")
    #expect(form.value("seed") == nil)
}

@Test func payloadCarriesPromptDefaultsAndSetValuesOnly() {
    var form = fluxForm()
    form.set("steps", to: .int(8))
    let payload = form.payload(prompt: "a lighthouse at dusk")
    #expect(
        payload
            == .object([
                "prompt": .string("a lighthouse at dusk"),
                "steps": .int(8),
                "guidance": .double(4.0),
                "size": .string("1024x1024"),
            ]))
}

@Test func setClampsIntAndFloatIntoSchemaRange() {
    var form = fluxForm()
    form.set("steps", to: .int(999))
    #expect(form.int("steps") == 50)
    form.set("steps", to: .int(-3))
    #expect(form.int("steps") == 1)
    form.set("guidance", to: .double(42.5))
    #expect(form.double("guidance") == 10)
    form.set("guidance", to: .int(3))
    #expect(form.double("guidance") == 3)
}

@Test func setRejectsUnknownKeysUnknownEnumValuesAndWrongTypes() {
    var form = fluxForm()
    form.set("cfg", to: .int(7))
    #expect(form.value("cfg") == nil)
    form.set("size", to: .string("4096x4096"))
    #expect(form.string("size") == "1024x1024")
    form.set("steps", to: .string("many"))
    #expect(form.int("steps") == 4)
}

@Test func rollDrawsAnIntWithinRangeAndChangesTheValue() {
    var form = fluxForm()
    form.roll("seed")
    guard let first = form.int("seed") else {
        Issue.record("roll left seed unset")
        return
    }
    #expect(first >= 0 && first < Int(UInt32.max))
    form.roll("seed")
    #expect(form.int("seed") != first)

    form.roll("steps")
    let steps = form.int("steps") ?? 0
    #expect((1...50).contains(steps))

    form.roll("size")
    #expect(form.string("size") == "1024x1024")
}

@Test func loadRoundTripsArtifactParamsBackIntoAPayload() {
    let original: JSONValue = .object([
        "prompt": .string("a lighthouse at dusk"),
        "steps": .int(12),
        "guidance": .double(2.5),
        "size": .string("512x512"),
        "seed": .int(771_342),
    ])
    var form = fluxForm()
    form.load(original)
    #expect(form.payload(prompt: "a lighthouse at dusk") == original)
}

@Test func loadResetsMissingKeysToDefaultsAndClearsMissingOptionals() {
    var form = fluxForm()
    form.set("steps", to: .int(30))
    form.set("seed", to: .int(7))
    form.load(.object(["prompt": .string("x"), "guidance": .double(1.0)]))
    #expect(form.int("steps") == 4)
    #expect(form.double("guidance") == 1.0)
    #expect(form.string("size") == "1024x1024")
    #expect(form.value("seed") == nil)
}

@Test func clearRemovesAValueSoPayloadOmitsIt() {
    var form = fluxForm()
    form.set("seed", to: .int(9))
    form.clear("seed")
    guard case .object(let fields) = form.payload(prompt: "x") else {
        Issue.record("expected object payload")
        return
    }
    #expect(fields["seed"] == nil)
}

@Test func specRangesDecodeFromMixedNumericJSON() {
    let spec = ParamSpec(
        key: "guidance", type: .float, defaultValue: .double(4.0),
        range: [.int(0), .double(10)])
    #expect(spec.doubleRange == 0...10)
    #expect(spec.intRange == 0...10)
    let unbounded = ParamSpec(key: "seed", type: .int)
    #expect(unbounded.intRange == nil)
    #expect(unbounded.isOptional)
    let bounded = ParamSpec(key: "steps", type: .int, defaultValue: .int(4))
    #expect(!bounded.isOptional)
}

@Test func floatStepAndPrecisionDeriveFromRangeWidth() {
    let guidance = ParamSpec(
        key: "guidance", type: .float, defaultValue: .double(4.0),
        range: [.int(0), .double(10)])
    #expect(guidance.doubleStep == 0.1)
    let strength = ParamSpec(
        key: "strength", type: .float, defaultValue: .double(0.1),
        range: [.double(0), .double(0.2)])
    #expect(strength.doubleStep == 0.001)
    let unbounded = ParamSpec(key: "noise", type: .float)
    #expect(unbounded.doubleStep == nil)
    #expect(ParamSpec.step(across: 0...100) == 1)
    #expect(ParamSpec.step(across: 5...5) == 1)
    #expect(ParamSpec.decimals(forStep: 0.1) == 1)
    #expect(ParamSpec.decimals(forStep: 0.001) == 3)
    #expect(ParamSpec.decimals(forStep: 1) == 0)
}

@Test func provenancePromptReadsOnlyStringPromptFields() {
    #expect(Provenance.prompt(of: .object(["prompt": .string("a lighthouse")])) == "a lighthouse")
    #expect(Provenance.prompt(of: .object(["steps": .int(4)])) == nil)
    #expect(Provenance.prompt(of: .string("a lighthouse")) == nil)
}

@Test func provenanceLineFollowsSchemaOrderAndSkipsPrompt() {
    let record = Fixtures.flux()
    let artifact = Artifact(
        id: "flux_abc",
        path: "2026/flux_abc.png",
        contentHash: "abc",
        model: record.name,
        modelID: record.id,
        runtime: "python:mflux",
        capability: .image,
        params: .object([
            "prompt": .string("a lighthouse at dusk"),
            "steps": .int(4),
            "guidance": .double(0.0),
            "size": .string("1024x1024"),
            "seed": .int(771_342),
        ]),
        createdAt: Date(),
        durationMs: 2140,
        jobID: "job-1")
    let line = Provenance.line(for: artifact, schema: record.params)
    #expect(
        line == "FLUX.1-schnell · steps 4 · guidance 0 · size 1024x1024 · seed 771342 · 2.1s")
}

@Test func provenanceLineWithoutSchemaSortsUnknownKeys() {
    let artifact = Artifact(
        id: "a",
        path: "2026/a.png",
        contentHash: "a",
        model: "M",
        modelID: "m",
        runtime: "r",
        capability: .image,
        params: .object([
            "prompt": .string("x"),
            "zeta": .int(1),
            "alpha": .string("v"),
        ]),
        createdAt: Date(),
        durationMs: 400,
        jobID: "j")
    #expect(Provenance.line(for: artifact) == "M · alpha v · zeta 1 · 400 ms")
}

@Test func provenanceDurationScalesUnits() {
    #expect(Provenance.duration(ms: 0) == "0 ms")
    #expect(Provenance.duration(ms: 999) == "999 ms")
    #expect(Provenance.duration(ms: 2140) == "2.1s")
    #expect(Provenance.duration(ms: 60_000) == "1m")
    #expect(Provenance.duration(ms: 72_000) == "1m 12s")
}

@Test func provenanceDetailsCarryPromptParamsDurationAndJob() {
    let record = Fixtures.flux()
    let artifact = Artifact(
        id: "flux_abc",
        path: "2026/flux_abc.png",
        contentHash: "abc",
        model: record.name,
        modelID: record.id,
        runtime: "python:mflux",
        capability: .image,
        params: .object([
            "prompt": .string("a lighthouse at dusk"),
            "steps": .int(4),
            "seed": .int(7),
        ]),
        createdAt: Date(),
        durationMs: 850,
        jobID: "job-1")
    let details = Provenance.details(for: artifact, schema: record.params)
    let expected = [
        "model: FLUX.1-schnell",
        "runtime: python:mflux",
        "capability: image",
        "prompt: a lighthouse at dusk",
        "steps: 4",
        "seed: 7",
        "duration: 850 ms",
        "job: job-1",
    ].joined(separator: "\n")
    #expect(details == expected)
}

@Test func failureDetailsIncludeErrorJobAndParams() {
    let details = Provenance.failureDetails(
        model: "FLUX.1-schnell",
        error: "sidecar crashed",
        jobID: "job-9",
        params: .object([
            "prompt": .string("a lighthouse"),
            "steps": .int(4),
        ]),
        schema: Fixtures.flux().params)
    let expected = [
        "model: FLUX.1-schnell",
        "error: sidecar crashed",
        "job: job-9",
        "prompt: a lighthouse",
        "steps: 4",
    ].joined(separator: "\n")
    #expect(details == expected)
}
