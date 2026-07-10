import Foundation

struct TranscriptionAudio: Sendable, Hashable {
    var samples: [Float]
    var sampleRate: Int

    init(samples: [Float], sampleRate: Int) {
        self.samples = samples
        self.sampleRate = sampleRate
    }

    static func from(payload: JSONValue) throws -> TranscriptionAudio {
        guard case .object(let fields) = payload else {
            throw KernelError.runtimeFailed("transcribe payload must be an object")
        }
        if case .string(let path)? = fields["audio"] {
            let expanded = (path as NSString).expandingTildeInPath
            return try fromWAVFile(URL(fileURLWithPath: expanded))
        }
        if case .string(let base64)? = fields["pcm"] {
            guard let data = Data(base64Encoded: base64) else {
                throw KernelError.runtimeFailed("transcribe pcm payload is not valid base64")
            }
            guard case .int(let rate)? = fields["sampleRate"], rate > 0 else {
                throw KernelError.runtimeFailed("transcribe pcm payload needs a sampleRate")
            }
            return TranscriptionAudio(samples: floatSamples(from: data), sampleRate: rate)
        }
        throw KernelError.runtimeFailed(
            "transcribe payload must carry an audio path or pcm frames")
    }

    static func fromWAVFile(_ url: URL) throws -> TranscriptionAudio {
        guard let data = try? Data(contentsOf: url) else {
            throw KernelError.runtimeFailed("could not read audio at \(url.path)")
        }
        return try fromWAVData(data)
    }

    static func fromWAVData(_ data: Data) throws -> TranscriptionAudio {
        guard data.count >= 12,
            data[0..<4] == Data("RIFF".utf8),
            data[8..<12] == Data("WAVE".utf8)
        else {
            throw KernelError.runtimeFailed("audio is not a RIFF WAVE file")
        }

        var offset = 12
        var format: (audioFormat: UInt16, channels: Int, sampleRate: Int, bitsPerSample: Int)?
        while offset + 8 <= data.count {
            let chunkID = data[offset..<offset + 4]
            let chunkSize = Int(readUInt32(data, at: offset + 4))
            let body = offset + 8
            guard body + chunkSize <= data.count else { break }

            if chunkID == Data("fmt ".utf8), chunkSize >= 16 {
                format = (
                    audioFormat: readUInt16(data, at: body),
                    channels: Int(readUInt16(data, at: body + 2)),
                    sampleRate: Int(readUInt32(data, at: body + 4)),
                    bitsPerSample: Int(readUInt16(data, at: body + 14))
                )
            }
            if chunkID == Data("data".utf8) {
                guard let format, format.channels > 0, format.sampleRate > 0 else {
                    throw KernelError.runtimeFailed("wave data appears before fmt chunk")
                }
                let payload = data.subdata(in: body..<body + chunkSize)
                let interleaved = try decodeSamples(payload, format: format)
                return TranscriptionAudio(
                    samples: downmixed(interleaved, channels: format.channels),
                    sampleRate: format.sampleRate)
            }
            offset = body + chunkSize + (chunkSize % 2)
        }
        throw KernelError.runtimeFailed("wave file has no data chunk")
    }

    func monoSamples(targetSampleRate: Int) -> [Float] {
        guard sampleRate != targetSampleRate, !samples.isEmpty, targetSampleRate > 0 else {
            return samples
        }
        let ratio = Double(sampleRate) / Double(targetSampleRate)
        let count = max(1, Int(Double(samples.count) / ratio))
        var resampled = [Float]()
        resampled.reserveCapacity(count)
        for index in 0..<count {
            let position = Double(index) * ratio
            let lower = min(Int(position), samples.count - 1)
            let upper = min(lower + 1, samples.count - 1)
            let fraction = Float(position - Double(lower))
            resampled.append(samples[lower] * (1 - fraction) + samples[upper] * fraction)
        }
        return resampled
    }

    static func floatSamples(from data: Data) -> [Float] {
        let count = data.count / MemoryLayout<Float>.size
        guard count > 0 else { return [] }
        return data.withUnsafeBytes { raw in
            (0..<count).map {
                Float(bitPattern: UInt32(
                    littleEndian: raw.loadUnaligned(
                        fromByteOffset: $0 * MemoryLayout<Float>.size, as: UInt32.self)))
            }
        }
    }

    private static func decodeSamples(
        _ payload: Data,
        format: (audioFormat: UInt16, channels: Int, sampleRate: Int, bitsPerSample: Int)
    ) throws -> [Float] {
        switch (format.audioFormat, format.bitsPerSample) {
        case (1, 16):
            let count = payload.count / 2
            return payload.withUnsafeBytes { raw in
                (0..<count).map {
                    Float(Int16(littleEndian: raw.loadUnaligned(
                        fromByteOffset: $0 * 2, as: Int16.self))) / Float(Int16.max)
                }
            }
        case (3, 32):
            return floatSamples(from: payload)
        default:
            throw KernelError.runtimeFailed(
                "unsupported wave encoding (format \(format.audioFormat), \(format.bitsPerSample)-bit)")
        }
    }

    private static func downmixed(_ interleaved: [Float], channels: Int) -> [Float] {
        guard channels > 1 else { return interleaved }
        let frames = interleaved.count / channels
        var mono = [Float]()
        mono.reserveCapacity(frames)
        for frame in 0..<frames {
            var sum: Float = 0
            for channel in 0..<channels {
                sum += interleaved[frame * channels + channel]
            }
            mono.append(sum / Float(channels))
        }
        return mono
    }

    private static func readUInt16(_ data: Data, at offset: Int) -> UInt16 {
        UInt16(data[data.startIndex + offset])
            | UInt16(data[data.startIndex + offset + 1]) << 8
    }

    private static func readUInt32(_ data: Data, at offset: Int) -> UInt32 {
        UInt32(data[data.startIndex + offset])
            | UInt32(data[data.startIndex + offset + 1]) << 8
            | UInt32(data[data.startIndex + offset + 2]) << 16
            | UInt32(data[data.startIndex + offset + 3]) << 24
    }
}
