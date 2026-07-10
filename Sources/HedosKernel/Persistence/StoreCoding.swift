import Foundation

enum StoreCoding {
    static let dateFormat = Date.ISO8601FormatStyle(includingFractionalSeconds: true)

    static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .custom { date, encoder in
            var container = encoder.singleValueContainer()
            try container.encode(date.formatted(dateFormat))
        }
        return encoder
    }

    static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let raw = try container.decode(String.self)
            guard
                let date = (try? Date(raw, strategy: dateFormat))
                    ?? (try? Date(raw, strategy: .iso8601))
            else {
                throw DecodingError.dataCorruptedError(
                    in: container, debugDescription: "Unparseable date \(raw)")
            }
            return date
        }
        return decoder
    }

    static func quarantine(_ url: URL) {
        let target = url.deletingLastPathComponent()
            .appendingPathComponent(
                "\(url.lastPathComponent).corrupt-\(Int(Date().timeIntervalSince1970))")
        try? FileManager.default.moveItem(at: url, to: target)
    }
}

extension Date {
    static func millisecondRounded() -> Date {
        Date(timeIntervalSince1970: (Date().timeIntervalSince1970 * 1000).rounded() / 1000)
    }
}
