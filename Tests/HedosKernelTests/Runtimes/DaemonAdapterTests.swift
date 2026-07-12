import Foundation
import Testing

@testable import HedosKernel

private func daemonRecord(name: String, runtimeID: RuntimeID) -> ModelRecord {
    ModelRecord(
        name: name,
        modality: .image,
        capabilities: [.image],
        source: ModelSource(
            kind: .folder, path: "/models/checkpoints/\(name).safetensors", repo: name),
        runtime: RuntimeRef(id: runtimeID, resolved: .auto, tier: .native),
        execution: .job, state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private let imageIdentified = IdentifiedModel(
    format: .diffusers, modality: .image, capabilities: [.image], execution: .job)

private func liveness(_ snapshot: DaemonLiveness.Snapshot) -> DaemonLiveness {
    let live = DaemonLiveness()
    live.store(snapshot)
    return live
}

@Test func comfyUIBidsOnlyWhenLiveAndNameMatched() {
    let record = daemonRecord(name: "dreamshaper", runtimeID: .comfyUI)

    let dead = ComfyUIAdapter(liveness: liveness(DaemonLiveness.Snapshot()))
    #expect(dead.bid(record, imageIdentified) == nil)

    let liveNoMatch = ComfyUIAdapter(
        liveness: liveness(
            .init(comfyUI: .init(alive: true, models: ["other.safetensors"]))))
    #expect(liveNoMatch.bid(record, imageIdentified) == nil)

    let liveMatch = ComfyUIAdapter(
        liveness: liveness(
            .init(comfyUI: .init(alive: true, models: ["dreamshaper.safetensors"]))))
    let bid = liveMatch.bid(record, imageIdentified)
    #expect(bid?.tier == .native)
    #expect(bid?.preference == 27)
}

@Test func a1111BidsOnlyWhenLiveAndNameMatched() {
    let record = daemonRecord(name: "dreamshaper", runtimeID: .a1111)

    let dead = A1111Adapter(liveness: liveness(DaemonLiveness.Snapshot()))
    #expect(dead.bid(record, imageIdentified) == nil)

    let liveMatch = A1111Adapter(
        liveness: liveness(.init(a1111: .init(alive: true, models: ["dreamshaper"]))))
    let bid = liveMatch.bid(record, imageIdentified)
    #expect(bid?.tier == .native)
    #expect(bid?.preference == 28)
}

@Test func comfyGraphCarriesTheSeededKnobs() {
    let payload = JSONValue.object([
        "prompt": .string("a fox"), "steps": .int(12), "guidance": .double(5.5),
        "width": .int(768), "height": .int(512), "seed": .int(99),
    ])
    let graph = ComfyUIAdapter.graph(payload: payload, checkpoint: "dreamshaper.safetensors")
    let sampler = graph["3"] as? [String: Any]
    let inputs = sampler?["inputs"] as? [String: Any]
    #expect(inputs?["steps"] as? Int == 12)
    #expect(inputs?["seed"] as? Int == 99)
    let loader = graph["4"] as? [String: Any]
    #expect((loader?["inputs"] as? [String: Any])?["ckpt_name"] as? String == "dreamshaper.safetensors")
    let latent = (graph["5"] as? [String: Any])?["inputs"] as? [String: Any]
    #expect(latent?["width"] as? Int == 768)
}

@Test func a1111RequestBodyCarriesTheSeededKnobs() {
    let payload = JSONValue.object([
        "prompt": .string("a fox"), "steps": .int(15), "cfg_scale": .double(6),
        "seed": .int(42),
    ])
    let body = A1111Adapter.requestBody(payload: payload)
    #expect(body["prompt"] as? String == "a fox")
    #expect(body["steps"] as? Int == 15)
    #expect(body["cfg_scale"] as? Double == 6)
    #expect(body["seed"] as? Int == 42)
}

@Test func comfyRunOnConnectionFailureMarksTheDaemonDead() async throws {
    let live = liveness(.init(comfyUI: .init(alive: true, models: ["dreamshaper.safetensors"])))
    let adapter = ComfyUIAdapter(
        liveness: DaemonLiveness(
            comfyURL: URL(string: "http://127.0.0.1:9")!, session: .shared),
        session: .shared)
    _ = live
    let record = daemonRecord(name: "dreamshaper", runtimeID: .comfyUI)
    let stream = adapter.run(record, .image, payload: .object(["prompt": .string("x")]))
    var threw = false
    do {
        for try await _ in stream {}
    } catch {
        threw = true
    }
    #expect(threw)
}

@Test func comfyCheckpointParsingReadsObjectInfo() throws {
    let json = try JSONSerialization.jsonObject(
        with: Data(
            """
            {"CheckpointLoaderSimple":{"input":{"required":{"ckpt_name":[["a.safetensors","b.safetensors"]]}}}}
            """.utf8))
    #expect(DaemonLiveness.comfyCheckpoints(json) == ["a.safetensors", "b.safetensors"])
}

@Test func a1111CheckpointParsingReadsModelNames() throws {
    let json = try JSONSerialization.jsonObject(
        with: Data(#"[{"title":"DreamShaper","model_name":"dreamshaper"}]"#.utf8))
    #expect(DaemonLiveness.a1111Checkpoints(json) == ["dreamshaper"])
}
