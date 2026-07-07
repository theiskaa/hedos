import Foundation
import Testing

@testable import HedosKernel

private func sineWave(samples: Int, amplitude: Float = 0.5) -> Data {
    var values = [Float]()
    values.reserveCapacity(samples)
    for index in 0..<samples {
        values.append(amplitude * sin(Float(index) * 0.1))
    }
    return values.withUnsafeBytes { Data($0) }
}

@Test func wavEncodingProducesValidHeaderAndSampleCount() {
    let pcm = sineWave(samples: 2400)
    let wav = SpeechAudio.wavData(fromFloat32: pcm, sampleRate: 24000)

    #expect(String(data: wav.prefix(4), encoding: .ascii) == "RIFF")
    #expect(String(data: wav.subdata(in: 8..<12), encoding: .ascii) == "WAVE")
    #expect(String(data: wav.subdata(in: 12..<16), encoding: .ascii) == "fmt ")
    #expect(String(data: wav.subdata(in: 36..<40), encoding: .ascii) == "data")
    #expect(wav.count == 44 + 2400 * 2)

    let dataSize = wav.subdata(in: 40..<44).withUnsafeBytes {
        $0.load(as: UInt32.self).littleEndian
    }
    #expect(dataSize == 2400 * 2)
    let rate = wav.subdata(in: 24..<28).withUnsafeBytes {
        $0.load(as: UInt32.self).littleEndian
    }
    #expect(rate == 24000)
}

@Test func peaksNormalizeAndBucketize() {
    let pcm = sineWave(samples: 28_000, amplitude: 0.3)
    let peaks = SpeechAudio.peaks(fromFloat32: pcm)

    #expect(peaks.count == 28)
    #expect(peaks.max() == 1.0)
    #expect(peaks.allSatisfy { $0 >= 0 && $0 <= 1 })

    let silence = Data(count: 4800 * 4)
    let flat = SpeechAudio.peaks(fromFloat32: silence)
    #expect(flat.count == 28)
    #expect(flat.allSatisfy { $0 == 0 })
}

@Test func durationDerivesFromSampleCount() {
    let pcm = sineWave(samples: 24000)
    #expect(SpeechAudio.durationMs(fromFloat32: pcm, sampleRate: 24000) == 1000)
    #expect(SpeechAudio.durationMs(fromFloat32: pcm, sampleRate: 0) == 0)
}

@Test func saveSpeechStoresWavArtifactWithProvenance() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])
    var record = Fixtures.gguf(path: "~/models/reader.gguf")
    record.modality = .speech
    record.capabilities = [.speak]
    record.runtime = RuntimeRef(id: "python:mlx-audio", resolved: .auto, tier: .managed)
    record.state = .ready
    try await kernel.registry.register(record)

    let pcm = sineWave(samples: 4800)
    let artifact = try await kernel.saveSpeech(
        modelID: record.id, voice: "af_heart", text: "hello there",
        sampleRate: 24000, pcm: pcm)

    #expect(artifact.capability == .speak)
    #expect(artifact.durationMs == 200)
    guard case .object(let fields) = artifact.params else {
        Issue.record("params not an object")
        return
    }
    #expect(fields["text"] == .string("hello there"))
    #expect(fields["voice"] == .string("af_heart"))
    if case .array(let peaks)? = fields["peaks"] {
        #expect(peaks.count == 28)
    } else {
        Issue.record("peaks missing")
    }

    let listed = try await kernel.artifacts()
    #expect(listed.contains { $0.id == artifact.id })
    let url = try await kernel.artifactURL(id: artifact.id)
    let header = try Data(contentsOf: #require(url)).prefix(4)
    #expect(String(data: header, encoding: .ascii) == "RIFF")
}
