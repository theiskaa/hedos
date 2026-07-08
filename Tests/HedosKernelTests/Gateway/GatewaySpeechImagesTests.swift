import Foundation
import Testing

@testable import HedosKernel

private func speakModel() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/kokoro-test.gguf")
    record.name = "kokoro-test"
    record.modality = .speech
    record.capabilities = [.speak]
    record.state = .ready
    return record
}

private func imageModel() -> ModelRecord {
    var record = Fixtures.flux()
    record.state = .ready
    return record
}

@Test func speechCollectsFramesIntoValidWAV() async throws {
    let pcm = Data((0..<400).flatMap { _ in [UInt8](repeating: 0, count: 4) })
    let port = FakeGatewayPort(
        records: [speakModel()],
        speakScript: [
            .audio(AudioFrame(data: pcm, sampleRate: 24000)),
            .audio(AudioFrame(data: pcm, sampleRate: 24000)),
            .done(nil),
        ],
        voicesList: ["af_heart", "am_adam"])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["model": "kokoro-test", "input": "hello there"])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/audio/speech"), token: stack.token, body: body))
    let http = response as! HTTPURLResponse
    #expect(http.statusCode == 200)
    #expect(http.value(forHTTPHeaderField: "Content-Type")?.contains("audio/wav") == true)
    #expect(data.count > 44)
    #expect(String(data: data.prefix(4), encoding: .ascii) == "RIFF")
    #expect(String(data: data.subdata(in: 8..<12), encoding: .ascii) == "WAVE")

    if case .object(let payload) = port.recorder.last!.payload {
        #expect(payload["text"] == .string("hello there"))
        #expect(payload["voice"] == .string("af_heart"))
    } else {
        Issue.record("speak payload should be an object")
    }
    await stack.stop()
}

@Test func speechRefusesNonWavFormats() async throws {
    let port = FakeGatewayPort(records: [speakModel()])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json([
        "model": "kokoro-test", "input": "hi", "response_format": "mp3",
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/audio/speech"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 400)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect((error?["message"] as? String)?.contains("wav") == true)
    await stack.stop()
}

@Test func imagesRunJobAndReturnB64PNG() async throws {
    let pngBytes = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3])
    let model = imageModel()
    let port = FakeGatewayPort(
        records: [model],
        jobResult: ["artifact-1"],
        artifacts: ["artifact-1": pngBytes])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json([
        "model": "FLUX.1-schnell", "prompt": "a quiet shelf", "size": "512x512",
    ])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/images/generations"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let images = object["data"] as! [[String: Any]]
    #expect(images.count == 1)
    let decoded = Data(base64Encoded: images[0]["b64_json"] as! String)
    #expect(decoded == pngBytes)

    let submission = port.recorder.last
    #expect(submission?.capability == .image)
    if case .object(let payload) = submission!.payload {
        #expect(payload["prompt"] == .string("a quiet shelf"))
        #expect(payload["size"] == .string("512x512"))
    } else {
        Issue.record("image payload should be an object")
    }
    await stack.stop()
}

@Test func imagesSurfaceJobFailuresHonestly() async throws {
    let port = FakeGatewayPort(records: [imageModel()], jobFailure: "diffusion exploded")
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["model": "FLUX.1-schnell", "prompt": "boom"])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/images/generations"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 500)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect((error?["message"] as? String)?.contains("diffusion exploded") == true)
    await stack.stop()
}

@Test func imagesRefuseMultiImageRequests() async throws {
    let port = FakeGatewayPort(records: [imageModel()], jobResult: ["a"])
    let stack = try await GatewayHarness.stack(
        port: port, routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["model": "FLUX.1-schnell", "prompt": "many", "n": 4])
    let (_, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/images/generations"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}

@Test func embeddingsAnswerHonest501() async throws {
    var embedder = Fixtures.gguf(path: "/tmp/hedos-fixtures/embedder.gguf")
    embedder.name = "embedder"
    embedder.capabilities = [.embed]
    embedder.state = .ready
    let stack = try await GatewayHarness.stack(
        port: FakeGatewayPort(records: [embedder]),
        routes: GatewayRouter.standardRoutes())
    let body = GatewayHarness.json(["model": "embedder", "input": "vectorize me"])
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/embeddings"), token: stack.token, body: body))
    #expect((response as! HTTPURLResponse).statusCode == 501)
    let object = try JSONSerialization.jsonObject(with: data) as! [String: Any]
    let error = object["error"] as? [String: Any]
    #expect(error?["code"] as? String == "capability_unsupported")
    #expect(object["data"] == nil)
    await stack.stop()
}
