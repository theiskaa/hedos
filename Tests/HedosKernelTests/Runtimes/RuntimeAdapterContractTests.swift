import Foundation
import Testing

@testable import HedosKernel

private let builtInRuntimeIDs: [RuntimeID] = [
    .llamaCpp, .whisperCpp, .ollama, .mlxSwift, .mlxLm, .mlxAudio,
    .mflux, .diffusers, .appleFoundation, .openAIEndpoint,
]

private func builtInAdapter(_ id: RuntimeID, registry: Registry) -> any RuntimeAdapter {
    switch id {
    case .llamaCpp: return LlamaCppAdapter()
    case .whisperCpp: return WhisperCppAdapter()
    case .ollama: return OllamaAdapter()
    case .mlxSwift: return MlxSwiftAdapter()
    case .mlxLm: return MlxLmAdapter()
    case .mlxAudio: return MlxAudioAdapter()
    case .mflux: return MfluxAdapter()
    case .diffusers: return DiffusersAdapter()
    case .appleFoundation: return AppleFoundationAdapter(registry: registry)
    case .openAIEndpoint:
        return OpenAIEndpointAdapter(secrets: InMemorySecretStore(), registry: registry)
    default:
        preconditionFailure("unmapped built-in runtime id \(id)")
    }
}

private let everyCapability: [Capability] = [
    .chat, .complete, .embed, .see, .image, .speak, .transcribe,
]

@Test(arguments: builtInRuntimeIDs)
func adapterDeclaresItsOwnTypedID(_ id: RuntimeID) throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let adapter = builtInAdapter(id, registry: Registry(directory: dir))
    #expect(adapter.id == id)
}

@Test(arguments: builtInRuntimeIDs)
func adapterNeverBidsOnAForeignFormat(_ id: RuntimeID) throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let adapter = builtInAdapter(id, registry: Registry(directory: dir))
    let foreign = IdentifiedModel(
        format: .unknown, modality: nil, capabilities: [], execution: .stream)
    #expect(adapter.bid(Fixtures.gguf(), foreign) == nil)
}

@Test(arguments: builtInRuntimeIDs)
func adapterNeverServesARecordBoundToAnotherRuntime(_ id: RuntimeID) throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let adapter = builtInAdapter(id, registry: Registry(directory: dir))
    var record = Fixtures.gguf()
    record.runtime = RuntimeRef(id: "someone-else", resolved: .auto, tier: .native)
    record.state = .ready
    for capability in everyCapability {
        #expect(!adapter.canServe(record, capability))
    }
}

@Test(arguments: builtInRuntimeIDs)
func adapterContextWindowIsNilOrPositive(_ id: RuntimeID) throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let adapter = builtInAdapter(id, registry: Registry(directory: dir))
    var record = Fixtures.gguf()
    record.contextLength = 8192
    let window = adapter.effectiveContextWindow(for: record, requested: nil)
    #expect(window == nil || window ?? 0 > 0)
    let clamped = adapter.effectiveContextWindow(for: record, requested: 512)
    #expect(clamped == nil || clamped ?? 0 > 0)
}

@Test func llamaAdapterBidMatrix() {
    let adapter = LlamaCppAdapter()
    let gguf = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat], execution: .stream)
    let safetensors = IdentifiedModel(
        format: .safetensors, modality: .text, capabilities: [.chat], execution: .stream)
    let record = Fixtures.gguf()
    #expect(adapter.bid(record, gguf) != nil)
    #expect(adapter.bid(record, gguf)?.tier == .native)
    #expect(adapter.bid(record, safetensors) == nil)
}

@Test func mlxAudioAdapterBidMatrix() {
    let adapter = MlxAudioAdapter()
    let speechMlx = IdentifiedModel(
        format: .safetensors, modality: .speech, capabilities: [.speak], execution: .stream)
    let speechUnknown = IdentifiedModel(
        format: .unknown, modality: .speech, capabilities: [.speak], execution: .stream)
    let textGguf = IdentifiedModel(
        format: .gguf, modality: .text, capabilities: [.chat], execution: .stream)

    let record = Fixtures.flux()
    #expect(adapter.bid(record, speechMlx)?.tier == .managed)
    #expect(adapter.bid(record, speechUnknown) == nil)
    #expect(adapter.bid(record, textGguf) == nil)
}
