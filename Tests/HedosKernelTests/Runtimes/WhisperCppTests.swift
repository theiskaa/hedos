import Foundation
import Testing

@testable import HedosKernel

private actor OptionsRecorder {
    private(set) var last: TranscriptionOptions?
    func record(_ options: TranscriptionOptions) { last = options }
}

private struct FakeWhisperBackend: WhisperBackend {
    let deltas: [String]
    var recorder: OptionsRecorder? = nil

    func load(path: String) async throws {}

    func unload() async {}

    func transcribe(samples: [Float], options: TranscriptionOptions)
        -> AsyncThrowingStream<TranscriptionSegment, Error>
    {
        let deltas = deltas
        let recorder = recorder
        return AsyncThrowingStream { continuation in
            Task {
                await recorder?.record(options)
                for delta in deltas {
                    continuation.yield(TranscriptionSegment(text: delta))
                }
                continuation.finish()
            }
        }
    }
}

private actor CallCounter {
    private var calls = 0

    func next() -> Int {
        calls += 1
        return calls
    }
}

private struct StallOnFirstCallBackend: WhisperBackend {
    let counter = CallCounter()

    func load(path: String) async throws {}

    func unload() async {}

    func transcribe(samples: [Float], options: TranscriptionOptions)
        -> AsyncThrowingStream<TranscriptionSegment, Error>
    {
        AsyncThrowingStream { continuation in
            let counter = counter
            let task = Task {
                let call = await counter.next()
                continuation.yield(TranscriptionSegment(text: "first"))
                if call == 1 {
                    do {
                        try await Task.sleep(for: .seconds(300))
                    } catch {
                        continuation.finish()
                        return
                    }
                }
                continuation.yield(TranscriptionSegment(text: "second"))
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

private func whisperRecord() -> ModelRecord {
    var record = Fixtures.gguf(path: "/tmp/hedos-tests/whisper-tiny.gguf")
    record.runtime = RuntimeRef(id: "whisper-cpp", resolved: .auto, tier: .native)
    return record
}

private func pcmPayload(
    samples: [Float] = [0.1, -0.2, 0.3, -0.4], sampleRate: Int = 16000
) -> JSONValue {
    var data = Data()
    for sample in samples {
        var little = sample.bitPattern.littleEndian
        withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
    }
    return .object([
        "pcm": .string(data.base64EncodedString()),
        "sampleRate": .int(sampleRate),
    ])
}

private func wavData(
    samples: [Float], sampleRate: Int, channels: Int, float32: Bool
) -> Data {
    var body = Data()
    if float32 {
        for sample in samples {
            var little = sample.bitPattern.littleEndian
            withUnsafeBytes(of: &little) { body.append(contentsOf: $0) }
        }
    } else {
        for sample in samples {
            var little = Int16(sample * Float(Int16.max)).littleEndian
            withUnsafeBytes(of: &little) { body.append(contentsOf: $0) }
        }
    }
    let bytesPerSample = float32 ? 4 : 2
    var data = Data("RIFF".utf8)
    var riffSize = UInt32(36 + body.count).littleEndian
    withUnsafeBytes(of: &riffSize) { data.append(contentsOf: $0) }
    data.append(Data("WAVEfmt ".utf8))
    var fields = Data()
    var fmtSize = UInt32(16).littleEndian
    withUnsafeBytes(of: &fmtSize) { fields.append(contentsOf: $0) }
    var audioFormat = UInt16(float32 ? 3 : 1).littleEndian
    withUnsafeBytes(of: &audioFormat) { fields.append(contentsOf: $0) }
    var channelCount = UInt16(channels).littleEndian
    withUnsafeBytes(of: &channelCount) { fields.append(contentsOf: $0) }
    var rate = UInt32(sampleRate).littleEndian
    withUnsafeBytes(of: &rate) { fields.append(contentsOf: $0) }
    var byteRate = UInt32(sampleRate * channels * bytesPerSample).littleEndian
    withUnsafeBytes(of: &byteRate) { fields.append(contentsOf: $0) }
    var blockAlign = UInt16(channels * bytesPerSample).littleEndian
    withUnsafeBytes(of: &blockAlign) { fields.append(contentsOf: $0) }
    var bits = UInt16(bytesPerSample * 8).littleEndian
    withUnsafeBytes(of: &bits) { fields.append(contentsOf: $0) }
    data.append(fields)
    data.append(Data("data".utf8))
    var dataSize = UInt32(body.count).littleEndian
    withUnsafeBytes(of: &dataSize) { data.append(contentsOf: $0) }
    data.append(body)
    return data
}

@Test func adapterStreamsDeltasThenDoneWithStats() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 1 << 20)
    let engine = WhisperEngine(backend: FakeWhisperBackend(deltas: ["hello", " world"]))
    let adapter = WhisperCppAdapter(governor: governor, engine: engine)
    let record = whisperRecord()

    var texts: [String] = []
    var stats: GenerationStats?
    var finished = false
    for try await chunk in adapter.invoke(record, .transcribe, payload: pcmPayload()) {
        switch chunk {
        case .text(let delta): texts.append(delta)
        case .done(let value):
            stats = value
            finished = true
        default: break
        }
    }

    #expect(texts == ["hello", " world"])
    #expect(finished)
    let done = try #require(stats)
    #expect(done.completionTokens == nil)
    #expect(done.durationMs != nil)
    #expect(await governor.isResident(record.id))
}

@Test func adapterDecodesLanguageAndTranslateFromPayload() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 1 << 20)
    let recorder = OptionsRecorder()
    let engine = WhisperEngine(
        backend: FakeWhisperBackend(deltas: ["ok"], recorder: recorder))
    let adapter = WhisperCppAdapter(governor: governor, engine: engine)
    let record = whisperRecord()

    var payload = pcmPayload()
    guard case .object(var fields) = payload else { return }
    fields["language"] = .string("es")
    fields["translate"] = .bool(true)
    payload = .object(fields)

    for try await _ in adapter.invoke(record, .transcribe, payload: payload) {}
    let options = try #require(await recorder.last)
    #expect(options.language == "es")
    #expect(options.translate == true)
}

@Test func cancelMidStreamReleasesEngineForNextRun() async throws {
    let governor = MemoryGovernor(totalMemoryMB: 1 << 20)
    let engine = WhisperEngine(backend: StallOnFirstCallBackend())
    let adapter = WhisperCppAdapter(governor: governor, engine: engine)
    let record = whisperRecord()

    var first: String?
    for try await chunk in adapter.invoke(record, .transcribe, payload: pcmPayload()) {
        if case .text(let delta) = chunk {
            first = delta
            break
        }
    }
    #expect(first == "first")

    var texts: [String] = []
    var stats: GenerationStats?
    for try await chunk in adapter.invoke(record, .transcribe, payload: pcmPayload()) {
        switch chunk {
        case .text(let delta): texts.append(delta)
        case .done(let value): stats = value
        default: break
        }
    }
    #expect(texts == ["first", "second"])
    #expect(stats != nil)
}

@Test func missingBackendSurfacesRuntimeUnavailable() async {
    let governor = MemoryGovernor(totalMemoryMB: 1 << 20)
    let engine = WhisperEngine(backend: MissingWhisperBackend())
    let adapter = WhisperCppAdapter(governor: governor, engine: engine)

    await #expect(throws: KernelError.self) {
        for try await _ in adapter.invoke(whisperRecord(), .transcribe, payload: pcmPayload()) {}
    }
}

@Test func emptyOrMalformedPayloadFails() async {
    let governor = MemoryGovernor(totalMemoryMB: 1 << 20)
    let engine = WhisperEngine(backend: FakeWhisperBackend(deltas: ["never"]))
    let adapter = WhisperCppAdapter(governor: governor, engine: engine)

    await #expect(throws: KernelError.self) {
        for try await _ in adapter.invoke(
            whisperRecord(), .transcribe, payload: .object([:])) {}
    }
    await #expect(throws: KernelError.self) {
        for try await _ in adapter.invoke(
            whisperRecord(), .transcribe,
            payload: .object(["pcm": .string("%%%")])) {}
    }
}

@Test func pcmPayloadRoundTripsFloatSamples() throws {
    let samples: [Float] = [0.5, -0.25, 0.125]
    let audio = try TranscriptionAudio.from(payload: pcmPayload(samples: samples))
    #expect(audio.samples == samples)
    #expect(audio.sampleRate == 16000)
    #expect(audio.monoSamples(targetSampleRate: 16000) == samples)
}

@Test func wavPCM16StereoDownmixesAndResamples() throws {
    let interleaved: [Float] = [0.5, -0.5, 0.25, -0.25, 1.0, 0.0, -1.0, 0.0]
    let data = wavData(samples: interleaved, sampleRate: 8000, channels: 2, float32: false)
    let audio = try TranscriptionAudio.fromWAVData(data)

    #expect(audio.sampleRate == 8000)
    #expect(audio.samples.count == 4)
    #expect(abs(audio.samples[0]) < 0.01)
    #expect(abs(audio.samples[2] - 0.5) < 0.01)

    let resampled = audio.monoSamples(targetSampleRate: 16000)
    #expect(resampled.count == 8)
}

@Test func wavFloat32MonoParsesFromFile() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let samples: [Float] = [0.1, 0.2, -0.3, 0.4]
    let url = dir.appendingPathComponent("clip.wav")
    try wavData(samples: samples, sampleRate: 16000, channels: 1, float32: true).write(to: url)

    let audio = try TranscriptionAudio.from(
        payload: .object(["audio": .string(url.path)]))
    #expect(audio.samples == samples)
    #expect(audio.sampleRate == 16000)
}

@Test func nonWavDataFailsCleanly() {
    #expect(throws: KernelError.self) {
        _ = try TranscriptionAudio.fromWAVData(Data("not a wave".utf8))
    }
}

@Test func sidecarBackendStreamsTextThroughTheFakeSidecar() async throws {
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
    let workdir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: workdir) }

    let supervisor = SidecarSupervisor()
    let backend = SidecarWhisperBackend(supervisor: supervisor) { path in
        SidecarSpec(
            runtimeID: "fake-whisper-\(path.hashValue)",
            executable: URL(fileURLWithPath: "/usr/bin/env"),
            arguments: ["python3", script.path, "normal"],
            workingDirectory: workdir,
            readyTimeout: .seconds(15))
    }

    try await backend.load(path: "/tmp/ggml-tiny.bin")
    var collected = ""
    for try await segment in backend.transcribe(
        samples: [Float](repeating: 0.25, count: 320), options: TranscriptionOptions())
    {
        collected += segment.text
    }
    #expect(collected == "heard 320 samples and understood")

    let leftovers = try FileManager.default.contentsOfDirectory(atPath: workdir.path)
        .filter { $0.hasSuffix(".f32") }
    #expect(leftovers.isEmpty)

    await backend.unload()
    #expect(await supervisor.isRunning("fake-whisper-\("/tmp/ggml-tiny.bin".hashValue)") == false)
}

private func fakeWhisperBackend(workdir: URL, supervisor: SidecarSupervisor)
    -> SidecarWhisperBackend
{
    let script = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Sidecar/FakeSidecar.py")
    return SidecarWhisperBackend(supervisor: supervisor) { path in
        SidecarSpec(
            runtimeID: "fake-whisper-\(path.hashValue)",
            executable: URL(fileURLWithPath: "/usr/bin/env"),
            arguments: ["python3", script.path, "normal"],
            workingDirectory: workdir,
            readyTimeout: .seconds(15))
    }
}

@Test func sidecarBackendForwardsLanguageAndTranslate() async throws {
    let workdir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: workdir) }
    let supervisor = SidecarSupervisor()
    let backend = fakeWhisperBackend(workdir: workdir, supervisor: supervisor)
    try await backend.load(path: "/tmp/ggml-tiny.bin")

    var collected = ""
    for try await segment in backend.transcribe(
        samples: [Float](repeating: 0.25, count: 320),
        options: TranscriptionOptions(language: "fr", translate: true))
    {
        collected += segment.text
    }
    #expect(collected.contains("lang=fr"))
    #expect(collected.contains("translate=True"))
    await supervisor.shutdownAll()
}

@Test func segmentTimestampsSurviveTheFrameProtocol() async throws {
    let workdir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: workdir) }
    let supervisor = SidecarSupervisor()
    let backend = fakeWhisperBackend(workdir: workdir, supervisor: supervisor)
    try await backend.load(path: "/tmp/ggml-tiny.bin")

    var segments: [TranscriptionSegment] = []
    for try await segment in backend.transcribe(
        samples: [Float](repeating: 0.25, count: 320), options: TranscriptionOptions())
    {
        segments.append(segment)
    }
    #expect(segments.count == 2)
    #expect(segments.allSatisfy { $0.startMs != nil && $0.endMs != nil })
    #expect(segments[0].startMs == 0)
    #expect(segments[0].endMs == 100)
    #expect(segments[1].startMs == 100)
    #expect(segments[1].endMs == 250)
    for segment in segments {
        #expect(segment.startMs! <= segment.endMs!)
    }
    await supervisor.shutdownAll()
}

@Test func whisperSpecDeclaresCooperativeCancelWithLongGrace() throws {
    let bundle = try #require(RuntimeBundle.directory(named: "python-whisper-cpp"))
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let spec = try SidecarWhisperBackend.makeSpec(
        path: "/tmp/ggml-tiny.bin", bundle: bundle,
        envDir: dir.appendingPathComponent("env"), runtimeID: "python:whisper-cpp",
        workdirRoot: dir)
    #expect(spec.cooperativeCancel == true)
    #expect(spec.cancelGraceTimeout == .seconds(30))
}

@Test func ggmlBinIdentifiesDiscoversAndBidsAsWhisper() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bin = dir.appendingPathComponent("ggml-tiny.bin")
    var payload = Data("lmgg".utf8)
    payload.append(Data(repeating: 0x01, count: 128))
    try payload.write(to: bin)
    let decoy = dir.appendingPathComponent("weights.bin")
    try Data(repeating: 0x02, count: 128).write(to: decoy)

    var record = Fixtures.gguf(path: bin.path)
    record.source = ModelSource(kind: .file, path: bin.path)
    record.primaryWeightPath = bin.path
    let identified = Identification.identify(record)
    #expect(identified.format == .ggmlBin)
    #expect(identified.modality == .audio)
    #expect(identified.capabilities == [.transcribe])

    let scanner = LooseFileScanner(directories: [dir])
    let result = await scanner.scan()
    #expect(result.discovered.map(\.name) == ["ggml-tiny"])
    #expect(result.discovered.first?.capabilitiesHint == [.transcribe])

    let adapter = WhisperCppAdapter(
        governor: MemoryGovernor(
            totalMemoryMB: 65536, heavyThresholdMB: 1024, defaultWarmWindow: .seconds(300)))
    let bid = adapter.bid(record, identified)
    #expect(bid?.tier == .managed)
}
