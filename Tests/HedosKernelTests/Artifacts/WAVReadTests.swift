import Foundation
import Testing

@testable import HedosKernel

@Test func wavRoundTripThroughFloat32PCM() throws {
    let samples: [Float] = (0..<2400).map { Float(sin(Double($0) * 0.05)) * 0.5 }
    let pcm = samples.withUnsafeBytes { Data($0) }
    let wav = SpeechAudio.wavData(fromFloat32: pcm, sampleRate: 24000)

    let decoded = SpeechAudio.float32PCM(fromWAV: wav)
    #expect(decoded != nil)
    #expect(decoded?.sampleRate == 24000)
    let decodedSamples = decoded!.pcm.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    #expect(decodedSamples.count == samples.count)
    for (original, roundTripped) in zip(samples, decodedSamples) {
        #expect(abs(original - roundTripped) < 0.001)
    }
}

@Test func wavReaderRejectsGarbage() {
    #expect(SpeechAudio.float32PCM(fromWAV: Data("not a wav".utf8)) == nil)
    #expect(SpeechAudio.float32PCM(fromWAV: Data(repeating: 0, count: 100)) == nil)
}
