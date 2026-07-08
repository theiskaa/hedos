import Foundation
import Testing

@testable import HedosKernel

private func record(
    id suffix: String, name: String, alias: String? = nil, state: ModelState = .ready
) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/\(suffix).gguf")
    record.name = name
    record.alias = alias
    record.state = state
    return record
}

@Test func resolverPrefersIDThenAliasThenName() throws {
    let byName = record(id: "a", name: "gemma3:latest")
    let byAlias = record(id: "b", name: "other-model", alias: "gemma-fast")
    let shelf = [byName, byAlias]

    #expect(try GatewayModelResolver.resolve(byName.id, shelf: shelf) == byName)
    #expect(try GatewayModelResolver.resolve("gemma-fast", shelf: shelf) == byAlias)
    #expect(try GatewayModelResolver.resolve("gemma3:latest", shelf: shelf) == byName)
}

@Test func resolverIsCaseInsensitiveOnNames() throws {
    let model = record(id: "a", name: "SmolLM-135M")
    #expect(try GatewayModelResolver.resolve("smollm-135m", shelf: [model]) == model)
}

@Test func resolverNormalizesLatestTagBothDirections() throws {
    let tagged = record(id: "a", name: "gemma3:latest")
    #expect(try GatewayModelResolver.resolve("gemma3", shelf: [tagged]) == tagged)

    let bare = record(id: "b", name: "phi4")
    #expect(try GatewayModelResolver.resolve("phi4:latest", shelf: [bare]) == bare)
}

@Test func resolverSkipsNotReadyRecords() throws {
    let missing = record(id: "a", name: "gone-model", state: .missing)
    #expect(throws: GatewayError.self) {
        try GatewayModelResolver.resolve("gone-model", shelf: [missing])
    }
}

@Test func resolverRefusesAmbiguityWithCandidateIDs() throws {
    let first = record(id: "a", name: "qwen3")
    let second = record(id: "b", name: "qwen3")
    do {
        _ = try GatewayModelResolver.resolve("qwen3", shelf: [first, second])
        Issue.record("ambiguity should throw")
    } catch let error as GatewayError {
        #expect(error.kind == .badRequest)
        #expect(error.message.contains(first.id))
        #expect(error.message.contains(second.id))
    }
}

@Test func resolverThrowsNotFoundForUnknownNames() throws {
    do {
        _ = try GatewayModelResolver.resolve("nothing-here", shelf: [])
        Issue.record("unknown model should throw")
    } catch let error as GatewayError {
        #expect(error.kind == .notFound)
    }
}
