import Foundation

public enum SpeechAudio {
    public static func wavData(fromFloat32 pcm: Data, sampleRate: Int) -> Data {
        let samples = int16Samples(fromFloat32: pcm)
        let dataSize = samples.count * 2
        var wav = Data(capacity: 44 + dataSize)
        wav.append(contentsOf: Array("RIFF".utf8))
        appendUInt32(&wav, UInt32(36 + dataSize))
        wav.append(contentsOf: Array("WAVE".utf8))
        wav.append(contentsOf: Array("fmt ".utf8))
        appendUInt32(&wav, 16)
        appendUInt16(&wav, 1)
        appendUInt16(&wav, 1)
        appendUInt32(&wav, UInt32(sampleRate))
        appendUInt32(&wav, UInt32(sampleRate * 2))
        appendUInt16(&wav, 2)
        appendUInt16(&wav, 16)
        wav.append(contentsOf: Array("data".utf8))
        appendUInt32(&wav, UInt32(dataSize))
        for sample in samples {
            appendUInt16(&wav, UInt16(bitPattern: sample))
        }
        return wav
    }

    public static func peaks(fromFloat32 pcm: Data, buckets: Int = 28) -> [Double] {
        let samples = floatSamples(fromFloat32: pcm)
        guard !samples.isEmpty, buckets > 0 else {
            return Array(repeating: 0, count: max(buckets, 0))
        }
        let bucketSize = max(1, samples.count / buckets)
        var result: [Double] = []
        result.reserveCapacity(buckets)
        for index in 0..<buckets {
            let start = index * bucketSize
            guard start < samples.count else {
                result.append(0)
                continue
            }
            let end = min(start + bucketSize, samples.count)
            var peak: Float = 0
            for sample in samples[start..<end] {
                peak = max(peak, abs(sample))
            }
            result.append(Double(min(peak, 1)))
        }
        let top = result.max() ?? 0
        guard top > 0 else { return result }
        return result.map { $0 / top }
    }

    public static func durationMs(fromFloat32 pcm: Data, sampleRate: Int) -> Int {
        guard sampleRate > 0 else { return 0 }
        let samples = pcm.count / MemoryLayout<Float>.size
        return samples * 1000 / sampleRate
    }

    private static func floatSamples(fromFloat32 pcm: Data) -> [Float] {
        let count = pcm.count / MemoryLayout<Float>.size
        return pcm.withUnsafeBytes { raw in
            Array(raw.bindMemory(to: Float.self).prefix(count))
        }
    }

    private static func int16Samples(fromFloat32 pcm: Data) -> [Int16] {
        floatSamples(fromFloat32: pcm).map { sample in
            let clamped = max(-1, min(1, sample))
            return Int16(clamped * Float(Int16.max))
        }
    }

    private static func appendUInt32(_ data: inout Data, _ value: UInt32) {
        withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
    }

    private static func appendUInt16(_ data: inout Data, _ value: UInt16) {
        withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) }
    }
}
