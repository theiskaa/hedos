import Foundation

public enum Frame: Equatable, Sendable {
    case control(JSONValue)
    case audio(Data)
}

public enum FrameCodecError: Error {
    case oversizedFrame(Int)
    case unknownType(UInt8)
    case malformedControl
}

public enum FrameCodec {
    public static let maxFrameBytes = 16 << 20

    public static func encode(_ frame: Frame) throws -> Data {
        var payload: Data
        var type: UInt8
        switch frame {
        case .control(let value):
            payload = try JSONEncoder().encode(value)
            type = 1
        case .audio(let data):
            payload = data
            type = 2
        }
        guard payload.count + 1 <= maxFrameBytes else {
            throw FrameCodecError.oversizedFrame(payload.count)
        }
        var length = UInt32(payload.count + 1).littleEndian
        var out = Data(bytes: &length, count: 4)
        out.append(type)
        out.append(payload)
        return out
    }

    public struct Decoder {
        private var buffer = Data()

        public init() {}

        public mutating func append(_ data: Data) throws -> [Frame] {
            buffer.append(data)
            var frames: [Frame] = []
            while buffer.count >= 4 {
                let length = buffer.prefix(4).withUnsafeBytes {
                    UInt32(littleEndian: $0.loadUnaligned(as: UInt32.self))
                }
                guard Int(length) <= FrameCodec.maxFrameBytes, length >= 1 else {
                    throw FrameCodecError.oversizedFrame(Int(length))
                }
                guard buffer.count >= 4 + Int(length) else { break }
                let body = buffer.subdata(in: 4..<(4 + Int(length)))
                buffer.removeSubrange(0..<(4 + Int(length)))

                let type = body[body.startIndex]
                let payload = body.dropFirst()
                switch type {
                case 1:
                    guard
                        let value = try? JSONDecoder().decode(JSONValue.self, from: Data(payload))
                    else { throw FrameCodecError.malformedControl }
                    frames.append(.control(value))
                case 2:
                    frames.append(.audio(Data(payload)))
                default:
                    throw FrameCodecError.unknownType(type)
                }
            }
            return frames
        }
    }
}

extension JSONValue {
    public var objectValue: [String: JSONValue]? {
        guard case .object(let fields) = self else { return nil }
        return fields
    }

    public var stringValue: String? {
        guard case .string(let value) = self else { return nil }
        return value
    }

    public var intValue: Int? {
        switch self {
        case .int(let value): return value
        case .double(let value): return Int(value)
        default: return nil
        }
    }

    public var doubleValue: Double? {
        switch self {
        case .double(let value): return value
        case .int(let value): return Double(value)
        default: return nil
        }
    }
}
