import Foundation

enum GatewayMultipart {
    struct Part: Sendable {
        var name: String?
        var filename: String?
        var data: Data
    }

    static func boundary(from contentType: String?) -> String? {
        guard let contentType,
            contentType.lowercased().contains("multipart/form-data")
        else { return nil }
        for component in contentType.split(separator: ";") {
            let trimmed = component.trimmingCharacters(in: .whitespaces)
            guard trimmed.lowercased().hasPrefix("boundary=") else { continue }
            var value = String(trimmed.dropFirst("boundary=".count))
            if value.hasPrefix("\""), value.hasSuffix("\""), value.count >= 2 {
                value = String(value.dropFirst().dropLast())
            }
            return value.isEmpty ? nil : value
        }
        return nil
    }

    static func parse(_ body: Data, boundary: String) -> [Part] {
        let delimiter = Data("--\(boundary)".utf8)
        var segments: [Data] = []
        var searchStart = body.startIndex
        var previousEnd: Data.Index?
        while let range = body.range(of: delimiter, in: searchStart..<body.endIndex) {
            if let previousEnd {
                segments.append(body.subdata(in: previousEnd..<range.lowerBound))
            }
            previousEnd = range.upperBound
            searchStart = range.upperBound
        }
        return segments.compactMap(part)
    }

    private static func part(_ segment: Data) -> Part? {
        let crlf = Data("\r\n".utf8)
        var slice = segment
        if slice.prefix(2) == Data("--".utf8) { return nil }
        if slice.prefix(2) == crlf { slice = slice.dropFirst(2) }
        guard let headerEnd = slice.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        let headerData = slice[slice.startIndex..<headerEnd.lowerBound]
        var content = slice[headerEnd.upperBound...]
        if content.suffix(2) == crlf { content = content.dropLast(2) }
        let (name, filename) = disposition(String(decoding: headerData, as: UTF8.self))
        return Part(name: name, filename: filename, data: Data(content))
    }

    private static func disposition(_ headers: String) -> (String?, String?) {
        var name: String?
        var filename: String?
        for line in headers.components(separatedBy: "\r\n") {
            guard line.lowercased().hasPrefix("content-disposition:") else { continue }
            for token in line.split(separator: ";") {
                let trimmed = token.trimmingCharacters(in: .whitespaces)
                if let value = fieldValue(trimmed, key: "name") { name = value }
                if let value = fieldValue(trimmed, key: "filename") { filename = value }
            }
        }
        return (name, filename)
    }

    private static func fieldValue(_ token: String, key: String) -> String? {
        let prefix = "\(key)="
        guard token.hasPrefix(prefix) else { return nil }
        var value = String(token.dropFirst(prefix.count))
        if value.hasPrefix("\""), value.hasSuffix("\""), value.count >= 2 {
            value = String(value.dropFirst().dropLast())
        }
        return value
    }
}
