import Foundation
import Testing

@testable import HedosKernel

private func writeJSON(_ json: String, in dir: URL, name: String) throws -> URL {
    let url = dir.appendingPathComponent(name)
    try Data(json.utf8).write(to: url)
    return url
}

@Test func architectureTableMatchesLegacyBehavior() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    let cases: [(json: String, modality: Modality?, capabilities: [Capability])] = [
        (#"{"architectures": ["KokoroForTTS"]}"#, .speech, [.speak]),
        (#"{"architectures": ["StyleTTS2Model"]}"#, .speech, [.speak]),
        (#"{"architectures": ["WhisperForConditionalGeneration"]}"#, .audio, [.transcribe]),
        (#"{"architectures": ["Qwen3ForCausalLM"]}"#, .text, [.chat, .complete]),
        (#"{"architectures": ["GPT2LMHeadModel"]}"#, .text, [.chat, .complete]),
        (DiscoveryFixtures.kokoroConfig, .speech, [.speak]),
        (DiscoveryFixtures.barkConfig, .speech, [.speak]),
        (DiscoveryFixtures.parlerTTSConfig, .speech, [.speak]),
        (DiscoveryFixtures.nomicEmbedConfig, .embedding, [.embed]),
        (#"{"architectures": ["XLMRobertaModel"]}"#, .embedding, [.embed]),
        (DiscoveryFixtures.qwen2VLConfig, .text, [.chat, .complete, .see]),
        (DiscoveryFixtures.mlxWhisperConfig, .audio, [.transcribe]),
    ]
    for (index, expected) in cases.enumerated() {
        let url = try writeJSON(expected.json, in: dir, name: "config-\(index).json")
        let hint = try #require(ModalityHints.fromConfigJSON(at: url))
        #expect(hint.modality == expected.modality)
        #expect(hint.capabilities == expected.capabilities)
    }

    let unmatched = try writeJSON(
        #"{"architectures": ["SomeVisionModel"]}"#, in: dir, name: "config-unmatched.json")
    #expect(ModalityHints.fromConfigJSON(at: unmatched) == nil)

    let visionConfigWithoutVLArchitecture = try writeJSON(
        #"{"architectures": ["SomeVisionModel"], "vision_config": {}}"#,
        in: dir, name: "config-vision-no-arch.json")
    #expect(ModalityHints.fromConfigJSON(at: visionConfigWithoutVLArchitecture) == nil)

    let vlArchitectureWithoutVisionConfig = try writeJSON(
        #"{"architectures": ["LlavaForConditionalGeneration"]}"#,
        in: dir, name: "config-arch-no-vision.json")
    #expect(ModalityHints.fromConfigJSON(at: vlArchitectureWithoutVisionConfig) == nil)
}

@Test func configJSONCarriesMaxPositionEmbeddings() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try writeJSON(
        #"{"architectures": ["Qwen3ForCausalLM"], "max_position_embeddings": 40960}"#,
        in: dir, name: "config.json")

    let hint = try #require(ModalityHints.fromConfigJSON(at: url))
    #expect(hint.modality == .text)
    #expect(hint.contextLength == 40960)
}

@Test func nPositionsFallbackApplies() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let url = try writeJSON(
        #"{"architectures": ["GPT2LMHeadModel"], "n_positions": 1024}"#,
        in: dir, name: "config.json")

    let hint = try #require(ModalityHints.fromConfigJSON(at: url))
    #expect(hint.contextLength == 1024)

    let windowless = try writeJSON(
        #"{"architectures": ["GPT2LMHeadModel"]}"#, in: dir, name: "bare.json")
    #expect(try #require(ModalityHints.fromConfigJSON(at: windowless)).contextLength == nil)
}

@Test func modelIndexHintConsultsFamilyRegistry() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }

    let flux = try writeJSON(DiscoveryFixtures.fluxModelIndex, in: dir, name: "flux.json")
    let fluxHint = ModalityHints.fromModelIndex(at: flux)
    #expect(fluxHint.modality == .image)
    #expect(fluxHint.capabilities == [.image])
    #expect(fluxHint.execution == .job)

    let video = try writeJSON(DiscoveryFixtures.cogVideoModelIndex, in: dir, name: "video.json")
    let videoHint = ModalityHints.fromModelIndex(at: video)
    #expect(videoHint.modality == .video)
    #expect(videoHint.capabilities.isEmpty)

    let unknown = try writeJSON(
        #"{"_class_name": "SomePipeline"}"#, in: dir, name: "unknown.json")
    let unknownHint = ModalityHints.fromModelIndex(at: unknown)
    #expect(unknownHint.modality == nil)
    #expect(unknownHint.capabilities.isEmpty)
}
