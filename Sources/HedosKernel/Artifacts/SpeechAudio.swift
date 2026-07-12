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

    public static func float32PCM(fromWAV wav: Data) -> (pcm: Data, sampleRate: Int)? {
        guard wav.count > 44,
            wav.prefix(4).elementsEqual("RIFF".utf8),
            wav.subdata(in: 8..<12).elementsEqual("WAVE".utf8)
        else { return nil }
        var offset = 12
        var format: UInt16 = 0
        var channels: UInt16 = 1
        var sampleRate: UInt32 = 0
        var bitsPerSample: UInt16 = 0
        var samplesData: Data?
        while offset + 8 <= wav.count {
            let chunkID = wav.subdata(in: offset..<(offset + 4))
            let chunkSize = Int(readUInt32(wav, at: offset + 4))
            let body = offset + 8
            guard body + chunkSize <= wav.count else { break }
            if chunkID.elementsEqual("fmt ".utf8), chunkSize >= 16 {
                format = readUInt16(wav, at: body)
                channels = readUInt16(wav, at: body + 2)
                sampleRate = readUInt32(wav, at: body + 4)
                bitsPerSample = readUInt16(wav, at: body + 14)
            } else if chunkID.elementsEqual("data".utf8) {
                samplesData = wav.subdata(in: body..<(body + chunkSize))
            }
            offset = body + chunkSize + (chunkSize % 2)
        }
        guard let samplesData, sampleRate > 0, channels >= 1 else { return nil }
        let channelCount = Int(channels)
        var floats: [Float] = []
        switch (format, bitsPerSample) {
        case (1, 16):
            let sampleCount = samplesData.count / 2
            floats.reserveCapacity(sampleCount / channelCount)
            samplesData.withUnsafeBytes { raw in
                let int16Buffer = raw.bindMemory(to: Int16.self)
                var index = 0
                while index < int16Buffer.count {
                    var sum: Float = 0
                    for channel in 0..<channelCount where index + channel < int16Buffer.count {
                        sum += Float(Int16(littleEndian: int16Buffer[index + channel])) / 32768
                    }
                    floats.append(sum / Float(channelCount))
                    index += channelCount
                }
            }
        case (3, 32):
            let sampleCount = samplesData.count / 4
            floats.reserveCapacity(sampleCount / channelCount)
            samplesData.withUnsafeBytes { raw in
                let floatBuffer = raw.bindMemory(to: Float.self)
                var index = 0
                while index < floatBuffer.count {
                    var sum: Float = 0
                    for channel in 0..<channelCount where index + channel < floatBuffer.count {
                        sum += floatBuffer[index + channel]
                    }
                    floats.append(sum / Float(channelCount))
                    index += channelCount
                }
            }
        case (1, 8):
            floats = Self.mixInterleaved(
                samplesData, bytesPerSample: 1, channels: channelCount) { bytes in
                (Float(Int(bytes[0]) - 128)) / 128
            }
        case (1, 24):
            floats = Self.mixInterleaved(
                samplesData, bytesPerSample: 3, channels: channelCount) { bytes in
                var value = Int32(bytes[0]) | Int32(bytes[1]) << 8 | Int32(bytes[2]) << 16
                if value & 0x80_0000 != 0 { value |= Int32(bitPattern: 0xFF00_0000) }
                return Float(value) / 8_388_608
            }
        case (1, 32):
            floats = Self.mixInterleaved(
                samplesData, bytesPerSample: 4, channels: channelCount) { bytes in
                let value =
                    Int32(bytes[0]) | Int32(bytes[1]) << 8 | Int32(bytes[2]) << 16
                    | Int32(bytes[3]) << 24
                return Float(value) / 2_147_483_648
            }
        default:
            return nil
        }
        return (floats.withUnsafeBytes { Data($0) }, Int(sampleRate))
    }

    static func mixInterleaved(
        _ data: Data, bytesPerSample: Int, channels: Int, decode: ([UInt8]) -> Float
    ) -> [Float] {
        let frameBytes = bytesPerSample * channels
        guard frameBytes > 0 else { return [] }
        let bytes = [UInt8](data)
        var floats: [Float] = []
        floats.reserveCapacity(bytes.count / frameBytes)
        var offset = 0
        while offset + frameBytes <= bytes.count {
            var sum: Float = 0
            for channel in 0..<channels {
                let start = offset + channel * bytesPerSample
                sum += decode(Array(bytes[start..<start + bytesPerSample]))
            }
            floats.append(sum / Float(channels))
            offset += frameBytes
        }
        return floats
    }

    private static func readUInt16(_ data: Data, at offset: Int) -> UInt16 {
        UInt16(data[data.startIndex + offset])
            | (UInt16(data[data.startIndex + offset + 1]) << 8)
    }

    private static func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[data.startIndex + offset])
            | (UInt32(data[data.startIndex + offset + 1]) << 8)
            | (UInt32(data[data.startIndex + offset + 2]) << 16)
            | (UInt32(data[data.startIndex + offset + 3]) << 24)
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
