struct Utf8StreamAssembler {
    private var tail: [UInt8] = []

    mutating func feed(_ bytes: some Sequence<UInt8>) -> String {
        var buffer = tail
        buffer.append(contentsOf: bytes)
        tail = []
        guard !buffer.isEmpty else { return "" }
        let held = Self.incompleteSuffixLength(of: buffer)
        if held > 0 {
            tail = Array(buffer.suffix(held))
            buffer.removeLast(held)
        }
        guard !buffer.isEmpty else { return "" }
        return String(decoding: buffer, as: UTF8.self)
    }

    mutating func flush() -> String {
        guard !tail.isEmpty else { return "" }
        let remainder = String(decoding: tail, as: UTF8.self)
        tail = []
        return remainder
    }

    private static func expectedLength(of lead: UInt8) -> Int? {
        switch lead {
        case 0x00...0x7F: return 1
        case 0xC2...0xDF: return 2
        case 0xE0...0xEF: return 3
        case 0xF0...0xF4: return 4
        default: return nil
        }
    }

    private static func incompleteSuffixLength(of buffer: [UInt8]) -> Int {
        for back in 1...min(3, buffer.count) {
            let byte = buffer[buffer.count - back]
            if byte & 0xC0 == 0x80 { continue }
            guard let length = expectedLength(of: byte), length > back else { return 0 }
            return back
        }
        return 0
    }
}
