import Foundation
import Testing

@testable import HedosKernel

private func completeModel(capabilities: [Capability] = [.chat, .complete]) -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-fixtures/doors.gguf")
    record.name = "doors:latest"
    record.state = .ready
    record.runtime.id = .llamaCpp
    record.capabilities = capabilities
    return record
}

private func doorsStack(
    port: FakeGatewayPort
) async throws -> GatewayStack {
    try await GatewayHarness.stack(port: port, routes: GatewayRouter.standardRoutes())
}

private func postJSON(_ stack: GatewayStack, _ path: String, _ body: [String: Any]) async throws
    -> (Int, Data)
{
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url(path), token: stack.token, body: GatewayHarness.json(body)))
    return ((response as! HTTPURLResponse).statusCode, data)
}

private func object(_ data: Data) -> [String: Any] {
    (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] ?? [:]
}

@Test func completionsPromptReturnsTextCompletion() async throws {
    let port = FakeGatewayPort(
        records: [completeModel()],
        chatScript: [.text("2+2="), .text("4"), .done(GenerationStats(completionTokens: 2))])
    let stack = try await doorsStack(port: port)
    let (status, data) = try await postJSON(
        stack, "/v1/completions",
        ["model": "doors:latest", "prompt": "solve: "])
    #expect(status == 200)
    let body = object(data)
    #expect(body["object"] as? String == "text_completion")
    let choices = body["choices"] as! [[String: Any]]
    #expect(choices[0]["text"] as? String == "2+2=4")
    #expect(choices[0]["finish_reason"] as? String == "stop")
    if case .object(let payload)? = port.recorder.last?.payload {
        #expect(payload["prompt"] == .string("solve: "))
    }
    await stack.stop()
}

@Test func completionsRejectsEchoBestOfAndPromptArray() async throws {
    let stack = try await doorsStack(port: FakeGatewayPort(records: [completeModel()]))
    let (echo, _) = try await postJSON(
        stack, "/v1/completions",
        ["model": "doors:latest", "prompt": "x", "echo": true])
    #expect(echo == 400)
    let (bestOf, _) = try await postJSON(
        stack, "/v1/completions",
        ["model": "doors:latest", "prompt": "x", "best_of": 3])
    #expect(bestOf == 400)
    let (promptArray, _) = try await postJSON(
        stack, "/v1/completions",
        ["model": "doors:latest", "prompt": ["a", "b"]])
    #expect(promptArray == 400)
    await stack.stop()
}

@Test func completionsStreamEmitsUsageWhenRequested() async throws {
    let port = FakeGatewayPort(
        records: [completeModel()],
        chatScript: [.text("hi"), .done(GenerationStats(promptTokens: 2, completionTokens: 1))])
    let stack = try await doorsStack(port: port)
    let (data, _) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/v1/completions"), token: stack.token,
            body: GatewayHarness.json([
                "model": "doors:latest", "prompt": "x", "stream": true,
                "stream_options": ["include_usage": true],
            ])))
    let events = String(data: data, encoding: .utf8)!.split(separator: "\n")
        .filter { $0.hasPrefix("data: ") }.map { String($0.dropFirst(6)) }
    let usageChunk = events.dropLast().compactMap {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any]
    }.first { $0["usage"] != nil }
    #expect((usageChunk?["usage"] as? [String: Any])?["prompt_tokens"] as? Int == 2)
    await stack.stop()
}

@Test func ollamaGenerateStreamsResponseAndDone() async throws {
    let port = FakeGatewayPort(
        records: [completeModel()],
        chatScript: [.text("hel"), .text("lo"), .done(GenerationStats(completionTokens: 2))])
    let stack = try await doorsStack(port: port)
    let (data, response) = try await URLSession.shared.data(
        for: GatewayHarness.request(
            "POST", stack.url("/api/generate"), token: stack.token,
            body: GatewayHarness.json(["model": "doors:latest", "prompt": "say hi"])))
    #expect((response as! HTTPURLResponse).statusCode == 200)
    let lines = String(data: data, encoding: .utf8)!.split(separator: "\n").map {
        (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any] ?? [:]
    }
    let responses = lines.compactMap { $0["response"] as? String }
    #expect(responses.joined() == "hello")
    #expect(lines.last?["done"] as? Bool == true)
    #expect(lines.last?["done_reason"] as? String == "stop")
    await stack.stop()
}

@Test func ollamaGenerateRejectsRawAndSystem() async throws {
    let stack = try await doorsStack(port: FakeGatewayPort(records: [completeModel()]))
    let (raw, _) = try await postJSON(
        stack, "/api/generate",
        ["model": "doors:latest", "prompt": "x", "raw": true])
    #expect(raw == 400)
    await stack.stop()
}

@Test func ollamaEmbedReturnsEmbeddingsAndLegacyReturnsSingle() async throws {
    let port = FakeGatewayPort(
        records: [completeModel(capabilities: [.embed])],
        embedScript: [.vector([0.1, 0.2, 0.3]), .done(GenerationStats(promptTokens: 4))])
    let stack = try await doorsStack(port: port)
    let (status, data) = try await postJSON(
        stack, "/api/embed",
        ["model": "doors:latest", "input": "vectorize"])
    #expect(status == 200)
    let body = object(data)
    #expect((body["embeddings"] as? [[Double]])?.first?.count == 3)
    #expect(body["prompt_eval_count"] as? Int == 4)

    let (legacyStatus, legacyData) = try await postJSON(
        stack, "/api/embeddings",
        ["model": "doors:latest", "prompt": "vectorize"])
    #expect(legacyStatus == 200)
    #expect((object(legacyData)["embedding"] as? [Double])?.count == 3)
    await stack.stop()
}

@Test func ollamaEmbedRejectsTruncateAndDimensions() async throws {
    let stack = try await doorsStack(
        port: FakeGatewayPort(records: [completeModel(capabilities: [.embed])]))
    let (truncate, _) = try await postJSON(
        stack, "/api/embed",
        ["model": "doors:latest", "input": "x", "truncate": true])
    #expect(truncate == 400)
    let (dimensions, _) = try await postJSON(
        stack, "/api/embed",
        ["model": "doors:latest", "input": "x", "dimensions": 128])
    #expect(dimensions == 400)
    await stack.stop()
}

@Test func transcriptionsMultipartReturnsText() async throws {
    let port = FakeGatewayPort(
        records: [completeModel(capabilities: [.transcribe])],
        chatScript: [.text("hello "), .text("world"), .done(GenerationStats())])
    let stack = try await doorsStack(port: port)
    let boundary = "hedosBoundary123"
    var body = Data()
    body.append(Data("--\(boundary)\r\n".utf8))
    body.append(
        Data("Content-Disposition: form-data; name=\"model\"\r\n\r\ndoors:latest\r\n".utf8))
    body.append(Data("--\(boundary)\r\n".utf8))
    body.append(
        Data(
            "Content-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n".utf8))
    body.append(Data("Content-Type: audio/wav\r\n\r\n".utf8))
    body.append(tinyWAV())
    body.append(Data("\r\n--\(boundary)--\r\n".utf8))

    var request = URLRequest(url: URL(string: stack.url("/v1/audio/transcriptions"))!)
    request.httpMethod = "POST"
    request.setValue("Bearer \(stack.token)", forHTTPHeaderField: "Authorization")
    request.setValue(
        "multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
    request.httpBody = body
    let (data, response) = try await URLSession.shared.data(for: request)
    #expect((response as! HTTPURLResponse).statusCode == 200)
    #expect(object(data)["text"] as? String == "hello world")
    await stack.stop()
}

@Test func transcriptionsRejectUnsupportedLanguageField() async throws {
    let stack = try await doorsStack(
        port: FakeGatewayPort(records: [completeModel(capabilities: [.transcribe])]))
    let boundary = "hedosBoundary123"
    var body = Data()
    body.append(Data("--\(boundary)\r\n".utf8))
    body.append(
        Data("Content-Disposition: form-data; name=\"model\"\r\n\r\ndoors:latest\r\n".utf8))
    body.append(Data("--\(boundary)\r\n".utf8))
    body.append(
        Data("Content-Disposition: form-data; name=\"language\"\r\n\r\nen\r\n".utf8))
    body.append(Data("--\(boundary)\r\n".utf8))
    body.append(
        Data("Content-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\n".utf8))
    body.append(tinyWAV())
    body.append(Data("\r\n--\(boundary)--\r\n".utf8))

    var request = URLRequest(url: URL(string: stack.url("/v1/audio/transcriptions"))!)
    request.httpMethod = "POST"
    request.setValue("Bearer \(stack.token)", forHTTPHeaderField: "Authorization")
    request.setValue(
        "multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
    request.httpBody = body
    let (_, response) = try await URLSession.shared.data(for: request)
    #expect((response as! HTTPURLResponse).statusCode == 400)
    await stack.stop()
}

private func tinyWAV() -> Data {
    let samples: [Float] = [0, 0.1, -0.1, 0.2]
    var pcm = Data()
    for sample in samples {
        var little = sample.bitPattern.littleEndian
        withUnsafeBytes(of: &little) { pcm.append(contentsOf: $0) }
    }
    var data = Data("RIFF".utf8)
    var riffSize = UInt32(36 + pcm.count).littleEndian
    withUnsafeBytes(of: &riffSize) { data.append(contentsOf: $0) }
    data.append(Data("WAVEfmt ".utf8))
    var fmtSize = UInt32(16).littleEndian
    withUnsafeBytes(of: &fmtSize) { data.append(contentsOf: $0) }
    var audioFormat = UInt16(3).littleEndian
    withUnsafeBytes(of: &audioFormat) { data.append(contentsOf: $0) }
    var channels = UInt16(1).littleEndian
    withUnsafeBytes(of: &channels) { data.append(contentsOf: $0) }
    var rate = UInt32(16000).littleEndian
    withUnsafeBytes(of: &rate) { data.append(contentsOf: $0) }
    var byteRate = UInt32(16000 * 4).littleEndian
    withUnsafeBytes(of: &byteRate) { data.append(contentsOf: $0) }
    var blockAlign = UInt16(4).littleEndian
    withUnsafeBytes(of: &blockAlign) { data.append(contentsOf: $0) }
    var bits = UInt16(32).littleEndian
    withUnsafeBytes(of: &bits) { data.append(contentsOf: $0) }
    data.append(Data("data".utf8))
    var dataSize = UInt32(pcm.count).littleEndian
    withUnsafeBytes(of: &dataSize) { data.append(contentsOf: $0) }
    data.append(pcm)
    return data
}
