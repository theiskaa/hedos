import Foundation

extension Identification {
    static func hasGGUFMagic(at url: URL) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let magic = try? handle.read(upToCount: 4)
        else { return false }
        try? handle.close()
        return magic == Data("GGUF".utf8)
    }

    static func hasGGMLMagic(at url: URL) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let magic = try? handle.read(upToCount: 4)
        else { return false }
        try? handle.close()
        return magic == Data("lmgg".utf8)
    }

    static let ggufArchitectureProfiles: [String: GGUFArchitectureProfile] = [
        "whisper": GGUFArchitectureProfile(
            modality: .audio,
            capabilities: [.transcribe],
            execution: .stream),
        "qwen2vl": GGUFArchitectureProfile(
            modality: .text,
            capabilities: [.chat, .complete, .see],
            execution: .stream),
        "mllama": GGUFArchitectureProfile(
            modality: .text,
            capabilities: [.chat, .complete, .see],
            execution: .stream),
        "clip": GGUFArchitectureProfile(
            modality: .vision,
            capabilities: [],
            execution: .sync),
        "bert": GGUFArchitectureProfile(
            modality: .embedding,
            capabilities: [.embed],
            execution: .stream),
        "nomic-bert": GGUFArchitectureProfile(
            modality: .embedding,
            capabilities: [.embed],
            execution: .stream),
    ]

    static let ollamaChatProfile = GGUFArchitectureProfile(
        modality: .text, capabilities: [.chat, .complete], execution: .stream)

    static let ollamaVisionProfile = GGUFArchitectureProfile(
        modality: .text, capabilities: [.chat, .complete, .see], execution: .stream)

    static func ggufGeneralArchitecture(at url: URL) -> String? {
        ggufFacts(at: url)?.architecture
    }

    static func ggufFacts(at url: URL) -> GGUFFacts? {
        guard let reader = GGUFHeaderReader(path: url.path) else { return nil }
        guard let magic = reader.readBytes(4), magic.elementsEqual(Array("GGUF".utf8)),
            let version: UInt32 = readLittleEndian(reader), version >= 2,
            let _: UInt64 = readLittleEndian(reader),
            let keyValueCount: UInt64 = readLittleEndian(reader)
        else { return nil }

        var architecture: String?
        var contextLengths: [String: Int] = [:]
        var hasChatTemplate = false

        walk: for _ in 0..<min(keyValueCount, 512) {
            guard let key = readGGUFString(reader),
                let valueType: UInt32 = readLittleEndian(reader)
            else { break walk }
            if key == "general.architecture" {
                guard valueType == 8, let value = readGGUFString(reader) else { break walk }
                architecture = value
            } else if key == "tokenizer.chat_template" {
                hasChatTemplate = true
                guard skipGGUFValue(reader, type: valueType) else { break walk }
            } else if key.hasSuffix(".context_length"),
                let value = readGGUFInteger(reader, type: valueType)
            {
                if value > 0 {
                    contextLengths[key] = value
                }
            } else {
                guard skipGGUFValue(reader, type: valueType) else { break walk }
            }
        }

        var contextLength: Int?
        if let architecture, let matched = contextLengths["\(architecture).context_length"] {
            contextLength = matched
        } else if contextLengths.count == 1 {
            contextLength = contextLengths.values.first
        }
        return GGUFFacts(
            architecture: architecture,
            contextLength: contextLength,
            hasChatTemplate: hasChatTemplate)
    }

    private final class GGUFHeaderReader {
        private let handle: FileHandle
        private var buffer: [UInt8] = []
        private var cursor = 0

        init?(path: String) {
            guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
            self.handle = handle
        }

        deinit {
            try? handle.close()
        }

        func readBytes(_ count: Int) -> ArraySlice<UInt8>? {
            guard count >= 0, fill(count) else { return nil }
            defer { cursor += count }
            return buffer[cursor..<cursor + count]
        }

        func skip(_ count: UInt64) -> Bool {
            let available = UInt64(buffer.count - cursor)
            if count <= available {
                cursor += Int(count)
                return true
            }
            let beyond = count - available
            buffer.removeAll(keepingCapacity: true)
            cursor = 0
            guard let current = try? handle.offset() else { return false }
            let (target, overflow) = current.addingReportingOverflow(beyond)
            guard !overflow else { return false }
            return (try? handle.seek(toOffset: target)) != nil
        }

        private func fill(_ count: Int) -> Bool {
            while buffer.count - cursor < count {
                guard let more = try? handle.read(upToCount: max(1 << 20, count)),
                    !more.isEmpty
                else { return false }
                if cursor > 0 {
                    buffer.removeFirst(cursor)
                    cursor = 0
                }
                buffer.append(contentsOf: more)
            }
            return true
        }
    }

    private static func readGGUFInteger(_ reader: GGUFHeaderReader, type: UInt32) -> Int? {
        switch type {
        case 4: (readLittleEndian(reader) as UInt32?).map { Int($0) }
        case 5: (readLittleEndian(reader) as Int32?).map { Int($0) }
        case 10: (readLittleEndian(reader) as UInt64?).map { Int(clamping: $0) }
        case 11: (readLittleEndian(reader) as Int64?).map { Int(clamping: $0) }
        default: nil
        }
    }

    private static func readLittleEndian<T: FixedWidthInteger>(
        _ reader: GGUFHeaderReader
    ) -> T? {
        let size = MemoryLayout<T>.size
        guard let bytes = reader.readBytes(size) else { return nil }
        var value: T = 0
        withUnsafeMutableBytes(of: &value) { $0.copyBytes(from: bytes) }
        return T(littleEndian: value)
    }

    private static func readGGUFString(_ reader: GGUFHeaderReader) -> String? {
        guard let length: UInt64 = readLittleEndian(reader), length <= 1 << 16,
            let bytes = reader.readBytes(Int(length))
        else { return nil }
        return String(decoding: bytes, as: UTF8.self)
    }

    private static func skipGGUFValue(_ reader: GGUFHeaderReader, type: UInt32) -> Bool {
        if let width = ggufScalarWidth(type: type) {
            return reader.skip(width)
        }
        switch type {
        case 8:
            guard let length: UInt64 = readLittleEndian(reader) else { return false }
            return reader.skip(length)
        case 9:
            guard let elementType: UInt32 = readLittleEndian(reader),
                let count: UInt64 = readLittleEndian(reader)
            else { return false }
            if let width = ggufScalarWidth(type: elementType) {
                let (total, overflow) = count.multipliedReportingOverflow(by: width)
                return !overflow && reader.skip(total)
            }
            guard elementType == 8, count <= 1 << 24 else { return false }
            for _ in 0..<count {
                guard let length: UInt64 = readLittleEndian(reader), reader.skip(length)
                else { return false }
            }
            return true
        default:
            return false
        }
    }

    private static func ggufScalarWidth(type: UInt32) -> UInt64? {
        switch type {
        case 0, 1, 7: 1
        case 2, 3: 2
        case 4, 5, 6: 4
        case 10, 11, 12: 8
        default: nil
        }
    }
}
